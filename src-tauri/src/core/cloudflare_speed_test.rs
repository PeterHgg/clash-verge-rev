use crate::{
    config::{Config, cloudflare_speed_test::*},
    core::{
        CoreManager,
        handle::Handle,
        notify::notification::{FrontendEvent, NotificationSystem},
    },
};
use anyhow::{Result, bail, ensure};
use parking_lot::Mutex;
use reqwest::Method;
use serde::de::DeserializeOwned;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::LazyLock,
    time::Duration,
};

#[derive(Clone)]
struct RunContext {
    profile_uid: String,
    fingerprint: String,
    network: String,
}

#[derive(Default)]
struct BridgeState {
    snapshot: Option<CloudflareSnapshot>,
    context: Option<RunContext>,
    context_run_id: Option<String>,
    polling: Option<String>,
}

static STATE: LazyLock<Mutex<BridgeState>> = LazyLock::new(Mutex::default);

fn network_identity() -> String {
    use network_interface::NetworkInterfaceConfig as _;
    let mut interfaces = network_interface::NetworkInterface::show().unwrap_or_default();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    format!("{interfaces:?}")
}

async fn context() -> Result<RunContext> {
    let runtime = Config::runtime().await.data_arc();
    let metadata = runtime
        .cloudflare_dns
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No committed profile is available"))?;
    Ok(RunContext {
        profile_uid: metadata.profile_uid.clone(),
        fingerprint: metadata.fingerprint.clone(),
        network: network_identity(),
    })
}

async fn request(method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
    let mut request = Handle::mihomo()
        .load_ctx()
        .build_request(method, path)
        .map_err(|_| anyhow::anyhow!("Could not prepare a request to the local Cloudflare interface"))?
        .timeout(Duration::from_secs(3));
    // Local-socket requests need the same credential as HTTP for this privileged API.
    if let Some(secret) = Config::runtime()
        .await
        .data_arc()
        .config
        .as_ref()
        .and_then(|config| config.get("secret"))
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|secret| !secret.is_empty())
    {
        request = request.bearer_auth(secret);
    }
    Ok(request)
}

async fn decode<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let limit = if status.is_success() { 256 * 1024 } else { 4096 };
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Could not read the local Cloudflare interface response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Cloudflare core response exceeds its size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        bail!("{} (HTTP {})", safe_http_error(status, &bytes), status.as_u16());
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("The core returned an invalid Cloudflare response"))
}

fn safe_http_error(status: reqwest::StatusCode, bytes: &[u8]) -> &'static str {
    if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return capability_http_reason(status);
    }
    let body = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let message = body
        .as_ref()
        .and_then(|body| body.get("message"))
        .and_then(|message| message.as_str());
    match message {
        Some("test source must be an HTTPS URL on port 443 without credentials or fragment") => {
            "Test source must be an HTTPS URL on port 443 without credentials or fragment"
        }
        Some("test source must have a public hostname" | "test source cannot be a private address") => {
            "Test source must have a public hostname or address"
        }
        Some("a benchmark is already running or stopping") => "A benchmark is already running or stopping",
        Some("benchmark run not found") => "The benchmark run no longer exists",
        Some(
            "benchmark options exceed the request size limit"
            | "invalid benchmark options"
            | "interface name is too long",
        ) => "The benchmark options are invalid or exceed their size limit",
        Some("physical interface is unavailable") => "The selected physical interface is unavailable",
        Some("tunnel, VPN or virtual interface cannot be used for a physical benchmark") => {
            "Select a physical Ethernet or Wi-Fi interface, not a tunnel or virtual interface"
        }
        Some("cannot verify a physical Ethernet or Wi-Fi interface" | "physical interface has no usable addresses") => {
            "A usable physical Ethernet or Wi-Fi interface could not be verified"
        }
        Some(
            "physical default route could not be verified"
            | "physical default route is unavailable; select an interface"
            | "physical default route has no local address"
            | "physical interface enumeration failed",
        ) => "The physical default route could not be verified; select a physical interface",
        Some("socket hook prevents verified physical interface binding") => {
            "A socket hook prevents verified physical interface binding"
        }
        Some("could not allocate benchmark ID") => "The core could not allocate a benchmark run ID",
        _ => match status {
            reqwest::StatusCode::NOT_FOUND => capability_http_reason(status),
            reqwest::StatusCode::CONFLICT => "A benchmark is already running or stopping",
            _ => "The local Cloudflare interface rejected the request",
        },
    }
}

fn capability_http_reason(status: reqwest::StatusCode) -> &'static str {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            "The local Cloudflare interface rejected authentication; this does not mean the core lacks the feature"
        }
        reqwest::StatusCode::NOT_FOUND => {
            "The running core has no Cloudflare test interface; use the bundled custom core"
        }
        _ => "The local Cloudflare interface returned an unsuccessful HTTP status",
    }
}

pub async fn capability() -> CloudflareCapability {
    let response: Result<reqwest::Response> =
        async { Ok(request(Method::GET, "/verge/cloudflare-test").await?.send().await?) }.await;
    let response = match response {
        Ok(response) => response,
        Err(_error) => {
            return CloudflareCapability::unsupported("Could not connect to the local core's Cloudflare interface");
        }
    };
    if !response.status().is_success() {
        return CloudflareCapability::unsupported(capability_http_reason(response.status()));
    }
    match decode::<CloudflareCapability>(response).await {
        Ok(mut capability) if capability.compatible() => {
            capability.reason = None;
            capability
        }
        Ok(_) => CloudflareCapability::unsupported("The core's Cloudflare capability version is incompatible"),
        Err(_) => CloudflareCapability::unsupported("The core returned an invalid Cloudflare capability response"),
    }
}

fn snapshot_from_value(value: serde_json::Value) -> Result<CloudflareSnapshot> {
    ensure!(
        value.get("method").and_then(serde_json::Value::as_str) == Some(METHOD),
        "Incompatible Cloudflare snapshot method"
    );
    serde_json::from_value(value).map_err(|_| anyhow::anyhow!("The core returned an invalid Cloudflare response"))
}

async fn current() -> Result<Option<CloudflareSnapshot>> {
    let value: Option<serde_json::Value> = decode(
        request(Method::GET, "/verge/cloudflare-test/jobs/current")
            .await?
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Could not connect to the local core's Cloudflare interface"))?,
    )
    .await?;
    let snapshot = value.map(snapshot_from_value).transpose()?;
    if let Some(snapshot) = &snapshot {
        validate_current(snapshot)?;
    }
    Ok(snapshot)
}

fn public_address(raw: &str) -> Result<IpAddr> {
    let ip: IpAddr = raw
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid Cloudflare candidate address"))?;
    ensure!(ip.to_string() == raw, "Cloudflare candidate address is not canonical");
    let public = match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(a, 0 | 10 | 127 | 224..=255)
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && ((b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99) || b == 168))
                && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] & 0xf000 == 0)
        }
    };
    ensure!(public, "Cloudflare candidate address is not public");
    Ok(ip)
}

fn validate_diagnostic(diagnostic: Option<&CloudflareDiagnostic>, error: Option<&str>) -> Result<()> {
    if let Some(diagnostic) = diagnostic {
        ensure!(
            diagnostic
                .http_status
                .is_none_or(|status| (100..=599).contains(&status))
                && diagnostic.retry_after_seconds.is_none_or(|seconds| seconds <= 86_400),
            "Invalid Cloudflare diagnostic details"
        );
    }
    if let Some(error) = error {
        let code = serde_json::from_value::<CloudflareDiagnosticCode>(serde_json::Value::String(error.into()))
            .map_err(|_| anyhow::anyhow!("Invalid Cloudflare legacy error code"))?;
        ensure!(
            diagnostic.is_none_or(|diagnostic| diagnostic.code == code),
            "Cloudflare diagnostic and legacy error disagree"
        );
    }
    Ok(())
}

fn usable_measurement(result: &CloudflareResult) -> bool {
    matches!(
        result.state,
        CloudflareResultState::Complete | CloudflareResultState::Short
    ) && result.successful_rounds >= 1
        && result.bytes_read > 0
        && result.duration_ms > 0
        && result.mean_rate.is_some_and(|rate| rate.is_finite() && rate > 0.0)
        && result.peak_rate.is_some_and(|rate| rate.is_finite() && rate > 0.0)
}

fn validate(snapshot: &CloudflareSnapshot) -> Result<()> {
    ensure!(
        snapshot.schema_version == 1
            && snapshot.capability == CAPABILITY
            && snapshot.method == METHOD
            && snapshot.latency_method == LATENCY_METHOD,
        "Incompatible Cloudflare snapshot"
    );
    ensure!(
        !snapshot.run_id.is_empty()
            && snapshot.run_id.len() <= 128
            && snapshot
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "Invalid Cloudflare run ID"
    );
    ensure!(
        snapshot.bytes_read <= snapshot.budget_bytes
            && snapshot.budget_bytes > 0
            && snapshot.budget_bytes <= BUDGET_BYTES,
        "Invalid Cloudflare byte budget"
    );
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&snapshot.started_at).is_ok(),
        "Invalid Cloudflare start timestamp"
    );
    ensure!(
        snapshot.results.len() <= 12
            && snapshot
                .sources
                .iter()
                .map(|source| source.name.as_str())
                .eq(PREFERRED_DOMAINS)
            && snapshot.total_targets as usize == snapshot.results.len()
            && snapshot.completed_targets <= snapshot.total_targets,
        "Invalid Cloudflare candidate or source count"
    );
    validate_diagnostic(snapshot.diagnostic.as_ref(), snapshot.error.as_deref())?;
    let mut membership: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for source in &snapshot.sources {
        ensure!(
            matches!(source.state.as_str(), "pending" | "resolved" | "failed") && source.ips.len() <= 4,
            "Invalid Cloudflare source state or address count"
        );
        validate_diagnostic(source.diagnostic.as_ref(), source.error.as_deref())?;
        let mut seen = BTreeSet::new();
        let mut ipv4 = 0;
        let mut ipv6 = 0;
        for raw in &source.ips {
            let ip = public_address(raw)?;
            ensure!(seen.insert(ip), "Duplicate Cloudflare source address");
            if ip.is_ipv4() {
                ipv4 += 1;
            } else {
                ipv6 += 1;
            }
            membership.entry(raw).or_default().push(&source.name);
        }
        ensure!(
            ipv4 <= 2
                && ipv6 <= 2
                && ipv4 == source.eligible_ipv4.min(2)
                && ipv6 == source.eligible_ipv6.min(2)
                && source.truncated == (source.eligible_ipv4 > 2 || source.eligible_ipv6 > 2)
                && (source.state == "resolved" || source.ips.is_empty())
                && (snapshot.stage == CloudflareStage::Resolving || source.state != "pending"),
            "Invalid Cloudflare source address accounting"
        );
    }
    let mut seen = BTreeSet::new();
    let mut bytes_read = 0_u64;
    for result in &snapshot.results {
        let ip = public_address(&result.ip)?;
        ensure!(seen.insert(ip), "Duplicate Cloudflare result address");
        ensure!(
            membership
                .get(result.ip.as_str())
                .is_some_and(|sources| { result.sources.iter().map(String::as_str).eq(sources.iter().copied()) }),
            "Cloudflare result does not match resolved source membership"
        );
        validate_diagnostic(result.diagnostic.as_ref(), result.error.as_deref())?;
        ensure!(
            [result.mean_rate, result.peak_rate, result.latency_ms]
                .into_iter()
                .flatten()
                .all(|value| value.is_finite() && value >= 0.0),
            "Invalid Cloudflare measurement"
        );
        ensure!(
            [result.stability, result.score]
                .into_iter()
                .flatten()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
            "Invalid Cloudflare score"
        );
        ensure!(
            result.failures <= result.attempts
                && result.attempts <= 4
                && result.successful_rounds <= result.attempted_rounds
                && result.attempted_rounds <= 3
                && result.duration_ms <= 8 * 60 * 1000,
            "Invalid Cloudflare measurement accounting"
        );
        if matches!(
            result.state,
            CloudflareResultState::Complete | CloudflareResultState::Short
        ) {
            ensure!(
                usable_measurement(result)
                    && (result.state != CloudflareResultState::Complete || result.successful_rounds >= 2),
                "Cloudflare result has no usable measured data"
            );
        } else {
            ensure!(
                result.successful_rounds == 0
                    && result.mean_rate.is_none()
                    && result.peak_rate.is_none()
                    && result.score.is_none()
                    && result.duration_ms == 0,
                "A Cloudflare result without successful rounds cannot have transfer measurements"
            );
        }
        bytes_read = bytes_read
            .checked_add(result.bytes_read)
            .ok_or_else(|| anyhow::anyhow!("Invalid Cloudflare measured byte count"))?;
    }
    ensure!(
        bytes_read <= snapshot.bytes_read,
        "Invalid Cloudflare measured byte count"
    );
    if snapshot.stage != CloudflareStage::Resolving {
        ensure!(
            membership.len() == snapshot.results.len(),
            "Cloudflare resolved candidates are missing results"
        );
    }
    Ok(())
}

fn validate_transition(previous: &CloudflareSnapshot, snapshot: &CloudflareSnapshot) -> Result<()> {
    if previous.run_id != snapshot.run_id {
        return Ok(());
    }
    ensure!(
        previous.started_at == snapshot.started_at
            && previous.interface_name == snapshot.interface_name
            && previous.dns_mode == snapshot.dns_mode
            && previous.url == snapshot.url
            && previous.budget_bytes == snapshot.budget_bytes,
        "Cloudflare run metadata changed"
    );
    for old in &previous.results {
        if let Some(new) = snapshot.results.iter().find(|result| result.ip == old.ip) {
            ensure!(
                new.successful_rounds >= old.successful_rounds && new.attempted_rounds >= old.attempted_rounds,
                "Cloudflare transfer round counts decreased"
            );
        }
    }
    if previous.stage != CloudflareStage::Resolving {
        ensure!(
            snapshot.stage != CloudflareStage::Resolving
                && previous.total_targets == snapshot.total_targets
                && previous.sources.iter().zip(&snapshot.sources).all(|(old, new)| {
                    old.name == new.name
                        && old.ips == new.ips
                        && old.eligible_ipv4 == new.eligible_ipv4
                        && old.eligible_ipv6 == new.eligible_ipv6
                        && old.truncated == new.truncated
                })
                && previous.results.len() == snapshot.results.len()
                && previous
                    .results
                    .iter()
                    .zip(&snapshot.results)
                    .all(|(old, new)| old.ip == new.ip && old.sources == new.sources),
            "Cloudflare resolved candidate membership changed"
        );
    }
    Ok(())
}

fn validate_current(snapshot: &CloudflareSnapshot) -> Result<()> {
    validate(snapshot)?;
    if let Some(previous) = &STATE.lock().snapshot {
        validate_transition(previous, snapshot)?;
    }
    Ok(())
}

fn publish(snapshot: CloudflareSnapshot) -> Result<()> {
    {
        let mut state = STATE.lock();
        if let Some(previous) = &state.snapshot {
            if previous.run_id == snapshot.run_id && previous.revision >= snapshot.revision {
                return Ok(());
            }
            if previous.run_id != snapshot.run_id
                && chrono::DateTime::parse_from_rfc3339(&previous.started_at).ok()
                    > chrono::DateTime::parse_from_rfc3339(&snapshot.started_at).ok()
            {
                return Ok(());
            }
            validate_transition(previous, &snapshot)?;
        }
        state.snapshot = Some(snapshot.clone());
    }
    if let Some(app) = crate::APP_HANDLE.get() {
        NotificationSystem::send_event(app.clone(), FrontendEvent::CloudflareSpeedTest { snapshot });
    }
    Ok(())
}

async fn cancel_or_unavailable(run_id: &str, reason: &str) {
    if cancel(run_id).await.is_ok() {
        return;
    }
    let latest = STATE
        .lock()
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.run_id == run_id)
        .cloned();
    if let Some(snapshot) = latest
        && let Some(app) = crate::APP_HANDLE.get()
    {
        NotificationSystem::send_event(
            app.clone(),
            FrontendEvent::CloudflareSpeedTestUnavailable {
                unavailable: CloudflareSpeedTestUnavailable {
                    run_id: snapshot.run_id,
                    revision: snapshot.revision,
                    reason: format!(
                        "{reason}. Cancellation could not be confirmed; the native deadline remains in force."
                    ),
                },
            },
        );
    }
}

fn poll(snapshot: &CloudflareSnapshot) {
    if snapshot.state != CloudflareRunState::Running {
        return;
    }
    let run_id = snapshot.run_id.clone();
    {
        let mut state = STATE.lock();
        if state.polling.as_ref() == Some(&run_id) {
            return;
        }
        state.polling = Some(run_id.clone());
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if STATE.lock().polling.as_ref() != Some(&run_id) {
                break;
            }
            let old_context = { STATE.lock().context.clone() };
            let changed = match (old_context, context().await) {
                (Some(old), Ok(new)) => {
                    old.profile_uid != new.profile_uid
                        || old.fingerprint != new.fingerprint
                        || old.network != new.network
                }
                (Some(_), Err(_)) => true,
                _ => false,
            };
            if changed || Handle::global().is_exiting() {
                cancel_or_unavailable(&run_id, "The active profile or network changed").await;
                break;
            }
            let policy = crate::utils::retry::RetryPolicy::fixed(
                std::num::NonZeroUsize::MIN.saturating_add(2),
                Duration::from_secs(1),
            );
            let result = crate::utils::retry::retry(policy, |_| async {
                current().await.map_err(crate::utils::retry::RetryError::Retry)
            })
            .await;
            match result {
                Ok(Some(snapshot)) if snapshot.run_id == run_id => {
                    let running = snapshot.state == CloudflareRunState::Running;
                    if publish(snapshot).is_err() {
                        cancel_or_unavailable(&run_id, "The core returned inconsistent candidate membership").await;
                        break;
                    }
                    if !running {
                        break;
                    }
                }
                Err(_) => {
                    cancel_or_unavailable(&run_id, "A valid core response could not be obtained after 3 attempts")
                        .await;
                    break;
                }
                _ => break,
            }
        }
        let mut state = STATE.lock();
        if state.polling.as_ref() == Some(&run_id) {
            state.polling = None;
        }
    });
}

fn benchmark_unavailable_reason(capability: &CloudflareCapability) -> &str {
    capability.reason.as_deref().unwrap_or(
        "The running core does not support the partial-result Cloudflare test method; use the bundled cf5 core",
    )
}

pub async fn get() -> Result<CloudflareSpeedTestView> {
    let _write = Config::lock_config_write().await;
    let mut capability = capability().await;
    let mut snapshot = if capability.supports_domain_only_test() {
        current().await?
    } else {
        capability.reason = Some(benchmark_unavailable_reason(&capability).into());
        capability.supported = false;
        None
    };
    if let Some(active) = snapshot
        .as_ref()
        .filter(|snapshot| snapshot.state == CloudflareRunState::Running)
    {
        let known_run = {
            let state = STATE.lock();
            state.context.is_some()
                && state.context_run_id.as_deref() == Some(active.run_id.as_str())
                && state
                    .snapshot
                    .as_ref()
                    .is_some_and(|previous| previous.run_id == active.run_id)
        };
        if !known_run {
            snapshot = Some(cancel(&active.run_id).await?);
        }
    }
    if let Some(snapshot) = &snapshot {
        publish(snapshot.clone())?;
        poll(snapshot);
    }
    Ok(CloudflareSpeedTestView { capability, snapshot })
}

pub async fn start(url: String, interface_name: Option<String>) -> Result<CloudflareSnapshot> {
    let _write = Config::lock_config_write().await;
    let manager = CoreManager::global();
    let _life = manager.lifecycle_lock.lock().await;
    let capability = capability().await;
    ensure!(
        capability.supports_domain_only_test(),
        "{}",
        benchmark_unavailable_reason(&capability)
    );
    let context = context().await?;
    let body = serde_json::json!({"url": url, "interfaceName": interface_name});
    let snapshot = snapshot_from_value(
        decode(
            request(Method::POST, "/verge/cloudflare-test/jobs")
                .await?
                .json(&body)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Could not connect to the local core's Cloudflare interface"))?,
        )
        .await?,
    )?;
    validate_current(&snapshot)?;
    {
        let mut state = STATE.lock();
        state.context = Some(context);
        state.context_run_id = Some(snapshot.run_id.clone());
    }
    publish(snapshot.clone())?;
    poll(&snapshot);
    Ok(snapshot)
}

pub async fn cancel(run_id: &str) -> Result<CloudflareSnapshot> {
    ensure!(
        !run_id.is_empty()
            && run_id.len() <= 128
            && run_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "Invalid Cloudflare run ID"
    );
    let snapshot = snapshot_from_value(
        decode(
            request(Method::DELETE, &format!("/verge/cloudflare-test/jobs/{run_id}"))
                .await?
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Could not connect to the local core's Cloudflare interface"))?,
        )
        .await?,
    )?;
    validate_current(&snapshot)?;
    publish(snapshot.clone())?;
    Ok(snapshot)
}

pub async fn cancel_active() {
    let run_id = STATE
        .lock()
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.state == CloudflareRunState::Running)
        .map(|snapshot| snapshot.run_id.clone());
    if let Some(run_id) = run_id {
        cancel_or_unavailable(&run_id, "The core configuration or lifecycle changed").await;
    }
}

fn owns_run_context(state: &BridgeState, run_id: &str, profile_uid: &str, current: &RunContext) -> bool {
    state.context_run_id.as_deref() == Some(run_id)
        && state.context.as_ref().is_some_and(|context| {
            context.profile_uid == profile_uid
                && context.profile_uid == current.profile_uid
                && context.network == current.network
                && context.fingerprint == current.fingerprint
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_authentication_failure_is_not_reported_as_missing_capability() {
        assert_ne!(
            capability_http_reason(reqwest::StatusCode::UNAUTHORIZED),
            capability_http_reason(reqwest::StatusCode::NOT_FOUND)
        );
        assert!(capability_http_reason(reqwest::StatusCode::UNAUTHORIZED).contains("authentication"));
        assert!(capability_http_reason(reqwest::StatusCode::NOT_FOUND).contains("no Cloudflare test interface"));
    }
    fn resolved_snapshot() -> CloudflareSnapshot {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "capability": CAPABILITY, "method": METHOD, "latencyMethod": LATENCY_METHOD,
            "runId": "run-1", "revision": 2,
            "state": "running", "stage": "screening", "interfaceName": "eth0", "dnsMode": "system",
            "url": "https://speed.cloudflare.com/…", "startedAt": "2026-10-19T00:00:00Z",
            "bytesRead": 0, "budgetBytes": BUDGET_BYTES, "completedTargets": 0, "totalTargets": 1,
            "results": [{"ip": "8.8.8.8", "sources": [PREFERRED_DOMAINS[0], PREFERRED_DOMAINS[1]],
                "state": "pending", "bytesRead": 0, "durationMs": 0, "failures": 0, "attempts": 0,
                "successfulRounds": 0, "attemptedRounds": 0}],
            "sources": [
                {"name": PREFERRED_DOMAINS[0], "ips": ["8.8.8.8"], "state": "resolved",
                    "eligibleIpv4": 1, "eligibleIpv6": 0, "truncated": false},
                {"name": PREFERRED_DOMAINS[1], "ips": ["8.8.8.8"], "state": "resolved",
                    "eligibleIpv4": 1, "eligibleIpv6": 0, "truncated": false},
                {"name": PREFERRED_DOMAINS[2], "ips": [], "state": "failed",
                    "eligibleIpv4": 0, "eligibleIpv6": 0, "truncated": false}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn old_or_unknown_latency_methods_cannot_be_accepted_as_tcp_connect_samples() {
        let snapshot = resolved_snapshot();
        validate(&snapshot).unwrap();
        let mut wire = serde_json::to_value(&snapshot).unwrap();
        wire.as_object_mut().unwrap().remove("latencyMethod");
        let mut old: CloudflareSnapshot = serde_json::from_value(wire).unwrap();
        assert!(validate(&old).is_err());
        old.latency_method = "elapsed-dial".into();
        assert!(validate(&old).is_err());
    }

    #[test]
    fn resolved_membership_is_public_deduplicated_and_frozen_after_resolution() {
        let original = resolved_snapshot();
        validate(&original).unwrap();
        let mut changed = original.clone();
        changed.revision += 1;
        changed.sources[0].ips[0] = "1.1.1.1".into();
        assert!(validate(&changed).is_err());
        changed = original.clone();
        changed.revision += 1;
        changed.results[0].ip = "1.1.1.1".into();
        for source in &mut changed.sources[..2] {
            source.ips[0] = "1.1.1.1".into();
        }
        validate(&changed).unwrap();
        assert!(validate_transition(&original, &changed).is_err());
        let mut initial = original.clone();
        initial.stage = CloudflareStage::Resolving;
        initial.results.clear();
        initial.total_targets = 0;
        for source in &mut initial.sources {
            source.ips.clear();
            source.eligible_ipv4 = 0;
            source.state = "pending".into();
        }
        validate(&initial).unwrap();
        validate_transition(&initial, &original).unwrap();
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:8.8.8.8",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(public_address(ip).is_err(), "accepted {ip}");
        }
        assert!(public_address("2606:4700:4700::1111").is_ok());
        let mut duplicate = original.clone();
        duplicate.results.push(duplicate.results[0].clone());
        duplicate.total_targets += 1;
        assert!(validate(&duplicate).is_err());
        let mut missing_origin = original;
        missing_origin.results[0].sources.pop();
        assert!(validate(&missing_origin).is_err());
    }

    #[test]
    fn short_samples_require_actual_positive_measurement_for_fixed_ip_trust() {
        let mut snapshot = resolved_snapshot();
        snapshot.results[0].state = CloudflareResultState::Short;
        snapshot.results[0].successful_rounds = 1;
        snapshot.results[0].attempted_rounds = 2;
        snapshot.results[0].peak_rate = Some(2048.0);
        snapshot.results[0].bytes_read = 512;
        snapshot.bytes_read = 512;
        assert!(!usable_measurement(&snapshot.results[0]));
        assert!(validate(&snapshot).is_err());
        snapshot.results[0].mean_rate = Some(1024.0);
        assert!(!usable_measurement(&snapshot.results[0]));
        snapshot.results[0].duration_ms = 500;
        assert!(usable_measurement(&snapshot.results[0]));
        validate(&snapshot).unwrap();
        snapshot.results[0].mean_rate = Some(0.0);
        assert!(!usable_measurement(&snapshot.results[0]));
        assert!(validate(&snapshot).is_err());
    }

    #[test]
    fn transfer_round_counts_match_results_and_never_decrease_within_a_run() {
        let mut screened = resolved_snapshot();
        screened.results[0].state = CloudflareResultState::Screened;
        screened.results[0].attempted_rounds = 1;
        validate(&screened).unwrap();
        let mut measured = screened.clone();
        measured.results[0].state = CloudflareResultState::Short;
        measured.results[0].successful_rounds = 1;
        measured.results[0].mean_rate = Some(1024.0);
        measured.results[0].peak_rate = Some(2048.0);
        measured.results[0].bytes_read = 512;
        measured.results[0].duration_ms = 500;
        measured.bytes_read = 512;
        validate(&measured).unwrap();
        validate_transition(&screened, &measured).unwrap();
        let mut in_flight = measured.clone();
        in_flight.results[0].attempted_rounds = 2;
        validate(&in_flight).unwrap();
        validate_transition(&measured, &in_flight).unwrap();
        assert!(validate_transition(&in_flight, &measured).is_err());
        assert!(validate_transition(&measured, &screened).is_err());
        for (successful, attempted, state) in [
            (2, 1, CloudflareResultState::Short),
            (1, 4, CloudflareResultState::Short),
            (0, 2, CloudflareResultState::Short),
            (1, 2, CloudflareResultState::Complete),
            (1, 2, CloudflareResultState::Failed),
        ] {
            let mut invalid = measured.clone();
            invalid.results[0].successful_rounds = successful;
            invalid.results[0].attempted_rounds = attempted;
            invalid.results[0].state = state;
            assert!(validate(&invalid).is_err());
        }
        measured.results[0].successful_rounds = 2;
        measured.results[0].attempted_rounds = 2;
        measured.results[0].state = CloudflareResultState::Complete;
        validate(&measured).unwrap();
        let mut failed = screened;
        failed.results[0].state = CloudflareResultState::Failed;
        failed.results[0].bytes_read = 512;
        failed.bytes_read = 512;
        validate(&failed).unwrap();
        failed.results[0].mean_rate = Some(0.0);
        assert!(validate(&failed).is_err());
    }

    #[test]
    fn snapshot_method_is_checked_before_required_transfer_counts_are_deserialized() {
        let mut wire = serde_json::to_value(resolved_snapshot()).unwrap();
        for field in ["successfulRounds", "attemptedRounds"] {
            let mut missing = wire.clone();
            missing["results"][0].as_object_mut().unwrap().remove(field);
            assert!(snapshot_from_value(missing).is_err());
        }
        for field in ["successfulRounds", "attemptedRounds"] {
            wire["results"][0].as_object_mut().unwrap().remove(field);
        }
        wire["method"] = "verge-cfst-transfer-v2".into();
        assert_eq!(
            snapshot_from_value(wire.clone()).unwrap_err().to_string(),
            "Incompatible Cloudflare snapshot method"
        );
        wire.as_object_mut().unwrap().remove("method");
        assert_eq!(
            snapshot_from_value(wire).unwrap_err().to_string(),
            "Incompatible Cloudflare snapshot method"
        );
    }

    #[test]
    fn controller_errors_and_diagnostics_do_not_forward_private_server_text() {
        let secret = "https://user:password@private.example/token?auth=secret";
        for status in [reqwest::StatusCode::BAD_REQUEST, reqwest::StatusCode::NOT_FOUND] {
            let body = serde_json::to_vec(&serde_json::json!({"message": secret, "code": secret})).unwrap();
            assert!(!safe_http_error(status, &body).contains(secret));
        }
        assert!(
            safe_http_error(
                reqwest::StatusCode::BAD_REQUEST,
                br#"{"message":"physical interface is unavailable"}"#,
            )
            .contains("physical interface")
        );
        assert!(safe_http_error(reqwest::StatusCode::UNAUTHORIZED, b"{}").contains("authentication"));
        assert_ne!(
            safe_http_error(reqwest::StatusCode::UNAUTHORIZED, b"{}"),
            safe_http_error(reqwest::StatusCode::NOT_FOUND, b"{}"),
        );
        let mut snapshot = resolved_snapshot();
        snapshot.results[0].error = Some(secret.into());
        assert!(validate(&snapshot).is_err());
        assert!(
            serde_json::from_value::<CloudflareDiagnostic>(serde_json::json!({
                "stage": "http", "code": secret
            }))
            .is_err()
        );
        snapshot.results[0].error = None;
        snapshot.results[0].diagnostic = Some(CloudflareDiagnostic {
            stage: CloudflareDiagnosticStage::Http,
            code: CloudflareDiagnosticCode::RateLimited,
            http_status: Some(429),
            content_type: None,
            retry_after_seconds: Some(86_401),
        });
        assert!(validate(&snapshot).is_err());
    }

    #[test]
    fn a_foreign_run_cannot_borrow_previous_profile_and_network_provenance() {
        let context = RunContext {
            profile_uid: "profile".into(),
            fingerprint: "source".into(),
            network: "network".into(),
        };
        let state = BridgeState {
            context: Some(context.clone()),
            context_run_id: Some("owned-run".into()),
            ..Default::default()
        };
        assert!(owns_run_context(&state, "owned-run", "profile", &context));
        assert!(!owns_run_context(&state, "foreign-run", "profile", &context));
        let changed = RunContext {
            network: "different-network".into(),
            ..context.clone()
        };
        assert!(!owns_run_context(&state, "owned-run", "profile", &changed));
        let changed = RunContext {
            profile_uid: "different-profile".into(),
            ..context.clone()
        };
        assert!(!owns_run_context(&state, "owned-run", "profile", &changed));
        let changed = RunContext {
            fingerprint: "different-source".into(),
            ..context
        };
        assert!(!owns_run_context(&state, "owned-run", "profile", &changed));
    }
}

pub async fn trusted_ip(ip: &str, run_id: &str, profile_uid: &str) -> Result<bool> {
    let current_context = context().await?;
    let state = STATE.lock();
    Ok(owns_run_context(&state, run_id, profile_uid, &current_context)
        && state.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.run_id == run_id
                && snapshot
                    .results
                    .iter()
                    .any(|result| result.ip == ip && usable_measurement(result))
        }))
}
