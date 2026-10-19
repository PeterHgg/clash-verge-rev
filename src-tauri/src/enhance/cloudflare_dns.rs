use crate::{
    config::{Config, IProfiles, cloudflare_speed_test::*},
    utils::dirs,
};
use anyhow::{Result, ensure};
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::{Component, Path},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone)]
pub struct CloudflareDnsRuntime {
    pub profile_uid: String,
    pub fingerprint: String,
    pub base: Mapping,
    pub domains: Vec<CloudflareDomain>,
    pub unknown_sources: Vec<String>,
    pub warnings: Vec<String>,
    pub active: bool,
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hostname(raw: &str) -> Option<String> {
    let raw = raw.trim_end_matches('.').to_ascii_lowercase();
    if raw.is_empty()
        || raw.len() > 253
        || raw.parse::<IpAddr>().is_ok()
        || !raw.contains('.')
        || raw == "localhost"
        || raw.ends_with(".localhost")
        || raw.ends_with(".local")
        || raw.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(raw)
}

fn remote_resolution(value: &Value) -> bool {
    value.get("dialer-proxy").is_some() || value.get("remote-dns-resolve").and_then(Value::as_bool) == Some(true)
}

fn scan_nodes(nodes: Option<&Value>, path: &str, conditional: bool, groups: &mut BTreeMap<String, CloudflareDomain>) {
    let Some(nodes) = nodes.and_then(Value::as_sequence) else {
        return;
    };
    for (index, node) in nodes.iter().enumerate() {
        let Some(raw) = node.get("server").and_then(Value::as_str) else {
            continue;
        };
        let Some(domain) = hostname(raw).or_else(|| raw.parse::<IpAddr>().ok().map(|ip| ip.to_string())) else {
            continue;
        };
        let conditional = conditional || remote_resolution(node);
        let bindable = hostname(&domain).is_some() && !conditional;
        let reason = if conditional {
            Some("Remote or dialer-proxy resolution cannot be guaranteed by local hosts".into())
        } else if !bindable {
            Some("IP literals and local/control destinations cannot be bound".into())
        } else {
            None
        };
        let path = format!("{path}[{index}].server");
        let occurrence = CloudflareOccurrence {
            id: digest(format!("{path}\0{domain}\0{raw}").as_bytes()),
            display: domain.clone(),
            field_path: path,
            role: CloudflareOccurrenceRole::ProxyServer,
            bindable,
            reason: reason.clone(),
        };
        let group = groups.entry(domain.clone()).or_insert_with(|| CloudflareDomain {
            domain,
            occurrences: Vec::new(),
            bindable: false,
            reason,
        });
        group.bindable |= bindable;
        if group.bindable {
            group.reason = None;
        }
        group.occurrences.push(occurrence);
    }
}

fn provider_remote_resolution(provider: &Value) -> bool {
    remote_resolution(provider)
        || provider.get("override").is_some_and(|value| {
            remote_resolution(value)
                || value
                    .get("override-expr")
                    .and_then(Value::as_sequence)
                    .is_some_and(|expressions| !expressions.is_empty())
        })
}

fn scan_inline_nodes(base: &Mapping, groups: &mut BTreeMap<String, CloudflareDomain>) {
    scan_nodes(base.get("proxies"), "proxies", false, groups);
    if let Some(providers) = base.get("proxy-providers").and_then(Value::as_mapping) {
        for (index, (_, provider)) in providers.iter().enumerate() {
            scan_nodes(
                provider.get("payload"),
                &format!("proxy-providers[{index}].payload"),
                provider_remote_resolution(provider),
                groups,
            );
        }
    }
}

fn exclude_controllers(base: &Mapping, groups: &mut BTreeMap<String, CloudflareDomain>) {
    for key in ["external-controller", "external-controller-tls"] {
        let controller = base
            .get(key)
            .and_then(Value::as_str)
            .and_then(|raw| tauri::Url::parse(&format!("http://{raw}")).ok())
            .and_then(|url| url.host_str().map(str::to_owned));
        if let Some(group) = controller.and_then(|domain| groups.get_mut(&domain)) {
            group.bindable = false;
            group.reason = Some("Controller destinations cannot be bound".into());
            for occurrence in &mut group.occurrences {
                occurrence.bindable = false;
                occurrence.reason = group.reason.clone();
            }
        }
    }
}

async fn provider_cache(root: &Path, raw: &str) -> Option<Vec<u8>> {
    let path = Path::new(raw);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(path)).await.ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
        return None;
    }
    use tokio::io::AsyncReadExt as _;
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes).await.ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    Some(bytes)
}

pub async fn collect(mut base: Mapping, profiles: &IProfiles) -> Result<CloudflareDnsRuntime> {
    let root = dirs::app_home_dir()?;
    crate::core::runtime_bundle::resolve_provider_path_conflicts(&mut base, &root)?;
    let profile_uid = profiles.current.as_deref().unwrap_or_default().to_owned();
    let mut groups = BTreeMap::new();
    let mut fingerprint_bytes = serde_yaml_ng::to_string(&base)?.into_bytes();
    fingerprint_bytes.extend_from_slice(profile_uid.as_bytes());
    scan_inline_nodes(&base, &mut groups);
    if let Ok(profile) = profiles.get_item(&profile_uid) {
        if let Some(url) = &profile.url {
            fingerprint_bytes.extend_from_slice(url.as_bytes());
        }
        fingerprint_bytes.extend_from_slice(format!("{:?}", profile.updated).as_bytes());
    }
    let mut total_bytes = 0;
    if let Some(providers) = base.get("proxy-providers").and_then(Value::as_mapping) {
        let mut count = 0;
        for (index, (_, provider)) in providers.iter().enumerate() {
            if provider.get("payload").is_some() {
                continue;
            }
            let label = format!("proxy-providers[{index}]");
            fingerprint_bytes.extend_from_slice(label.as_bytes());
            count += 1;
            let bytes = if count <= 32 && total_bytes < 8 * 1024 * 1024 {
                if let Some(path) = provider.get("path").and_then(Value::as_str) {
                    provider_cache(&root, path).await
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(bytes) = bytes {
                total_bytes += bytes.len();
                fingerprint_bytes.extend_from_slice(&Sha256::digest(&bytes));
                if let Ok(value) = serde_yaml_ng::from_slice::<Value>(&bytes) {
                    scan_nodes(
                        value.get("proxies"),
                        &format!("{label}.cache.proxies"),
                        provider_remote_resolution(provider),
                        &mut groups,
                    );
                }
            }
        }
    }
    exclude_controllers(&base, &mut groups);
    Ok(CloudflareDnsRuntime {
        profile_uid,
        fingerprint: digest(&fingerprint_bytes),
        base,
        domains: groups.into_values().collect(),
        unknown_sources: Vec::new(),
        active: false,
        warnings: Vec::new(),
    })
}

pub fn validate_eligibility(metadata: &CloudflareDnsRuntime, bindings: &[CloudflareDnsBinding]) -> Result<()> {
    for binding in bindings {
        ensure!(
            metadata.domains.iter().any(|domain| {
                domain.domain == binding.domain
                    && domain.bindable
                    && domain.occurrences.iter().any(|occurrence| {
                        occurrence.bindable && matches!(occurrence.role, CloudflareOccurrenceRole::ProxyServer)
                    })
            }),
            "The hostname is not a locally resolved proxy server in this committed profile"
        );
    }
    Ok(())
}

pub fn confirmation(metadata: &CloudflareDnsRuntime, settings: &ProfileCloudflareDnsSettings) -> Result<String> {
    Ok(digest(
        format!(
            "{}\0{}",
            metadata.fingerprint,
            serde_json::to_string(&settings.bindings)?
        )
        .as_bytes(),
    ))
}

pub fn conflicts(base: &Mapping, bindings: &[CloudflareDnsBinding]) -> Vec<String> {
    let Some(hosts) = base.get("hosts").and_then(Value::as_mapping) else {
        return Vec::new();
    };
    bindings
        .iter()
        .filter(|binding| {
            hosts
                .keys()
                .filter_map(Value::as_str)
                .any(|key| host_matches(key, &binding.domain))
        })
        .map(|binding| format!("{} already matches an existing hosts entry", binding.domain))
        .collect()
}

fn host_matches(pattern: &str, domain: &str) -> bool {
    let mut trie = HostsTrie::default();
    trie.insert(pattern, None);
    trie.search(&domain.split('.').rev().collect::<Vec<_>>(), &mut 4096)
        .ok()
        .flatten()
        .is_some()
}

#[derive(Default)]
struct HostsTrie {
    children: BTreeMap<String, HostsTrie>,
    value: Option<Option<String>>,
}

impl HostsTrie {
    fn insert(&mut self, pattern: &str, target: Option<String>) {
        let pattern = pattern.trim_end_matches('.').to_ascii_lowercase();
        if let Some(suffix) = pattern.strip_prefix("+.") {
            self.insert(suffix, target.clone());
            self.insert(&format!(".{suffix}"), target);
            return;
        }
        let mut node = self;
        for label in pattern.rsplit('.') {
            node = node.children.entry(label.into()).or_default();
        }
        node.value = Some(target);
    }

    fn search<'a>(&'a self, labels: &[&str], work: &mut usize) -> Result<Option<&'a Option<String>>> {
        ensure!(
            *work > 0,
            "The combined hosts alias graph exceeds its validation work limit"
        );
        *work -= 1;
        let Some((label, remaining)) = labels.split_first() else {
            return Ok(self.value.as_ref());
        };
        for key in [*label, "*"] {
            if let Some(child) = self.children.get(key)
                && let Some(value) = child.search(remaining, work)?
            {
                return Ok(Some(value));
            }
        }
        Ok(self.children.get("").and_then(|child| child.value.as_ref()))
    }
}

pub fn validate_hosts(hosts: &Mapping) -> Result<()> {
    let mut trie = HostsTrie::default();
    let mut aliases = Vec::new();
    for (key, value) in hosts {
        let Some(key) = key.as_str() else {
            continue;
        };
        let target = value
            .as_str()
            .map(|target| target.trim_end_matches('.').to_ascii_lowercase())
            .filter(|target| !target.is_empty() && target.parse::<IpAddr>().is_err());
        if let Some(target) = &target {
            aliases.push(target.clone());
        }
        trie.insert(key, target);
    }
    let mut work = 1_000_000;
    for mut target in aliases {
        let mut seen = BTreeSet::new();
        loop {
            ensure!(
                seen.len() < 64 && seen.insert(target.clone()),
                "The combined hosts aliases contain a cycle or exceed 64 levels"
            );
            match trie.search(&target.split('.').rev().collect::<Vec<_>>(), &mut work)? {
                Some(Some(next)) => target = next.clone(),
                _ => break,
            }
        }
    }
    Ok(())
}

pub fn overlay(mut config: Mapping, bindings: &[CloudflareDnsBinding]) -> Result<Mapping> {
    if bindings.is_empty() {
        return Ok(config);
    }
    let mut hosts = config
        .get("hosts")
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    for binding in bindings {
        ensure!(
            hostname(&binding.domain).as_deref() == Some(binding.domain.as_str()),
            "Invalid binding hostname"
        );
        match &binding.target {
            CloudflareBindingTarget::PreferredDomain { domain } => {
                ensure!(PREFERRED_DOMAINS.contains(&domain.as_str()), "Invalid preferred domain")
            }
            CloudflareBindingTarget::FixedIp { ip, .. } => {
                ip.parse::<IpAddr>()?;
            }
        }
        hosts.retain(|key, _| {
            !key.as_str()
                .is_some_and(|key| key.eq_ignore_ascii_case(&binding.domain))
        });
        hosts.insert(
            binding.domain.clone().into(),
            match &binding.target {
                CloudflareBindingTarget::PreferredDomain { domain } => Value::String(domain.clone()),
                CloudflareBindingTarget::FixedIp { ip, .. } => Value::Sequence(vec![Value::String(ip.clone())]),
            },
        );
    }
    validate_hosts(&hosts)?;
    config.insert("hosts".into(), hosts.into());
    let mut dns = config
        .get("dns")
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    dns.insert("use-hosts".into(), true.into());
    config.insert("dns".into(), dns.into());
    Ok(config)
}

type BinaryStamp = (std::path::PathBuf, u64, Option<SystemTime>);
static BINARY_SUPPORT: LazyLock<tokio::sync::Mutex<Option<(BinaryStamp, bool)>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

async fn binary_supported() -> bool {
    let core = Config::verge().await.latest_arc().get_valid_clash_core();
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let path = executable.with_file_name(format!("{core}{}", std::env::consts::EXE_SUFFIX));
    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return false;
    };
    let stamp = (path.clone(), metadata.len(), metadata.modified().ok());
    let mut cache = BINARY_SUPPORT.lock().await;
    if let Some((cached, supported)) = cache.as_ref().filter(|(cached, _)| *cached == stamp) {
        let _ = cached;
        return *supported;
    }
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(&path)
            .arg("-v")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let supported = output.ok().and_then(Result::ok).is_some_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).split_whitespace().any(|word| {
                matches!(
                    word,
                    "v1.19.32-verge-adaptive1-cf1"
                        | "v1.19.32-verge-adaptive1-cf2"
                        | "v1.19.32-verge-adaptive1-cf3"
                        | "v1.19.32-verge-adaptive1-cf4"
                        | "v1.19.32-verge-adaptive1-cf5"
                )
            })
    });
    *cache = Some((stamp, supported));
    supported
}

pub async fn apply(metadata: &mut CloudflareDnsRuntime, settings: &ProfileCloudflareDnsSettings) -> Result<Mapping> {
    if !settings.enabled || settings.bindings.is_empty() {
        return Ok(metadata.base.clone());
    }
    if validate_eligibility(metadata, &settings.bindings).is_err() {
        metadata.warnings.push(
            "Saved bindings are inactive because a hostname is no longer a locally resolved proxy server; remove invalid bindings before applying again".into(),
        );
        return Ok(metadata.base.clone());
    }
    let confirmed = settings.confirmation.as_deref() == Some(confirmation(metadata, settings)?.as_str());
    let supported = if matches!(
        *crate::core::CoreManager::global().get_running_mode(),
        crate::core::manager::RunningMode::NotRunning
    ) {
        let state = crate::core::runstate::RUN_STATE.state();
        let known_sidecar = !state.op_in_flight
            && (state.sidecar_allowed || matches!(state.health, crate::core::runstate::ServiceHealth::NotInstalled));
        known_sidecar && binary_supported().await
    } else {
        let capability = crate::core::cloudflare_speed_test::capability().await;
        capability.compatible() && capability.dns_aliases && capability.fixed_address_family
    };
    if !confirmed || !supported {
        metadata.warnings.push(
            if !confirmed {
                "Saved bindings are inactive because the profile sources changed; review and apply them again"
            } else {
                "Saved bindings are inactive because DNS binding support could not be confirmed for this core"
            }
            .into(),
        );
        return Ok(metadata.base.clone());
    }
    let config = overlay(metadata.base.clone(), &settings.bindings)?;
    metadata.active = true;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_only_collects_schema_proxy_servers_and_redacts_other_fields() -> Result<()> {
        let config: Mapping = serde_yaml_ng::from_str(
            r#"
proxies:
  - server: NODE.EXAMPLE.
    sni: identity.example
    password: secret
    nested: {server: nested.example, url: 'https://user:pass@download.example/token?auth=hidden#fragment'}
  - {server: node.example, dialer-proxy: upstream}
  - {server: remote.example, remote-dns-resolve: true}
  - {server: '127.0.0.1'}
  - {server: local.local}
  - {server: 'https://url-server.example/private'}
  - {server: control.example}
  - {server: tls-control.example}
external-controller: control.example:9090
external-controller-tls: tls-control.example:9091
proxy-providers:
  private-provider-name:
    type: inline
    url: https://provider.example/subscription
    health-check: {url: https://health.example/probe}
    payload:
      - {server: inline.example, ws-opts: {headers: {Host: host.example}}}
  remote-provider:
    type: inline
    override: {dialer-proxy: upstream}
    payload: [{server: provider-remote.example}]
  expression-provider:
    type: inline
    override: {override-expr: ['.server = "changed.example"']}
    payload: [{server: expression.example}]
rule-providers:
  private-rule-name:
    payload: [{server: rule.example}]
dns: {nameserver: ['https://dns.example/private?key=hidden']}
server: arbitrary.example
"#,
        )?;
        let mut groups = BTreeMap::new();
        scan_inline_nodes(&config, &mut groups);
        exclude_controllers(&config, &mut groups);
        let cache: Value = serde_yaml_ng::from_str(
            "proxies: [{server: cached.example, sni: cache-identity.example, nested: {server: cache-nested.example}}]\nserver: cache-arbitrary.example\npayload: [{server: cache-payload.example}]",
        )?;
        scan_nodes(
            cache.get("proxies"),
            "proxy-providers[3].cache.proxies",
            false,
            &mut groups,
        );
        let wire = serde_json::to_string(&groups)?;
        for excluded in [
            "user",
            "pass",
            "token",
            "secret",
            "hidden",
            "fragment",
            "private",
            "identity.example",
            "nested.example",
            "download.example",
            "url-server.example",
            "provider.example",
            "health.example",
            "host.example",
            "rule.example",
            "dns.example",
            "arbitrary.example",
            "changed.example",
            "cache-payload.example",
        ] {
            assert!(!wire.contains(excluded), "collected {excluded}");
        }
        assert!(groups["node.example"].bindable);
        assert_eq!(groups["node.example"].occurrences.len(), 2);
        assert!(groups["node.example"].occurrences[0].bindable);
        assert!(!groups["node.example"].occurrences[1].bindable);
        assert!(groups["inline.example"].bindable && groups["cached.example"].bindable);
        for domain in [
            "remote.example",
            "127.0.0.1",
            "provider-remote.example",
            "expression.example",
            "control.example",
            "tls-control.example",
        ] {
            assert!(!groups[domain].bindable);
        }
        assert!(
            groups
                .values()
                .flat_map(|domain| &domain.occurrences)
                .all(|occurrence| {
                    matches!(occurrence.role, CloudflareOccurrenceRole::ProxyServer)
                        && occurrence.field_path.ends_with(".server")
                })
        );
        Ok(())
    }

    #[tokio::test]
    async fn legacy_non_node_binding_stays_saved_but_cannot_activate_with_a_valid_old_token() -> Result<()> {
        let base: Mapping = serde_yaml_ng::from_str("url: https://download.example/private")?;
        let mut metadata = CloudflareDnsRuntime {
            profile_uid: "profile".into(),
            fingerprint: "source".into(),
            base: base.clone(),
            domains: Vec::new(),
            unknown_sources: Vec::new(),
            warnings: Vec::new(),
            active: false,
        };
        let mut settings = ProfileCloudflareDnsSettings {
            enabled: true,
            bindings: vec![CloudflareDnsBinding {
                domain: "download.example".into(),
                target: CloudflareBindingTarget::PreferredDomain {
                    domain: PREFERRED_DOMAINS[0].into(),
                },
            }],
            confirmation: None,
        };
        settings.confirmation = Some(confirmation(&metadata, &settings)?);
        let saved = serde_json::to_value(&settings)?;
        assert!(validate_eligibility(&metadata, &settings.bindings).is_err());
        assert_eq!(apply(&mut metadata, &settings).await?, base);
        assert!(!metadata.active);
        assert_eq!(serde_json::to_value(&settings)?, saved);
        assert!(validate_eligibility(&metadata, &[]).is_ok());
        settings.bindings.clear();
        settings.enabled = false;
        assert_eq!(apply(&mut metadata, &settings).await?, base);
        Ok(())
    }
    #[test]
    fn overlay_is_final_and_disabling_regenerates_exact_base() -> Result<()> {
        for dns in ["", "dns: {use-hosts: false}", "dns: {use-hosts: true}"] {
            let base: Mapping = serde_yaml_ng::from_str(&format!("hosts: {{ORIGIN.EXAMPLE: 192.0.2.1}}\n{dns}"))?;
            let binding = CloudflareDnsBinding {
                domain: "origin.example".into(),
                target: CloudflareBindingTarget::PreferredDomain {
                    domain: PREFERRED_DOMAINS[0].into(),
                },
            };
            let applied = overlay(base.clone(), &[binding])?;
            assert_eq!(applied["dns"]["use-hosts"], Value::Bool(true));
            assert_eq!(applied["hosts"].as_mapping().map(Mapping::len), Some(1));
            assert_eq!(
                applied["hosts"]["origin.example"],
                Value::String(PREFERRED_DOMAINS[0].into())
            );
            assert_eq!(overlay(base.clone(), &[])?, base);
        }
        Ok(())
    }
    #[test]
    fn combined_hosts_rejects_case_and_wildcard_cycles() -> Result<()> {
        for yaml in [
            "A.example: b.example\nb.example: A.example",
            "'*.example': target.other\ntarget.other: x.example",
            "'+.example': example",
        ] {
            assert!(validate_hosts(&serde_yaml_ng::from_str(yaml)?).is_err());
        }
        assert!(
            validate_hosts(&serde_yaml_ng::from_str(
                "'*.example': a.example\na.example: 192.0.2.1"
            )?)
            .is_ok()
        );
        assert!(validate_hosts(&serde_yaml_ng::from_str("'*.example': a.example")?).is_err());
        Ok(())
    }
}
