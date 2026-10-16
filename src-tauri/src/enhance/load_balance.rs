use anyhow::{Context as _, Result};
use serde_yaml_ng::{Mapping, Value};
use std::collections::{HashMap, HashSet};

const DEFAULT_TEST_URL: &str = "https://www.gstatic.com/generate_204";
const BUILTINS: &[&str] = &[
    "DIRECT",
    "REJECT",
    "REJECT-DROP",
    "PASS",
    "PASS-RULE",
    "COMPATIBLE",
    "GLOBAL",
];

#[derive(Clone, Debug)]
pub struct GroupPlan {
    pub name: String,
    pub original: Mapping,
}

#[derive(Clone, Debug)]
pub struct UnsupportedGroup {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct EnhancementPlan {
    pub groups: Vec<GroupPlan>,
    pub internal_names: HashSet<String>,
    pub unsupported: Vec<UnsupportedGroup>,
}

pub fn transform(mut config: Mapping, enabled: bool) -> Result<(Mapping, Option<EnhancementPlan>)> {
    if !enabled {
        return Ok((config, None));
    }

    let proxy_types: HashMap<_, _> = sequence(&config, "proxies")
        .iter()
        .filter_map(|proxy| Some((text(proxy, "name")?, text(proxy, "type")?)))
        .collect();
    let group_names: HashSet<_> = sequence(&config, "proxy-groups")
        .iter()
        .filter_map(|group| text(group, "name"))
        .collect();
    let mut plan = EnhancementPlan::default();
    let mut managed_providers = HashSet::new();
    let mut unmanaged_providers = HashSet::new();
    let mut groups = sequence(&config, "proxy-groups").to_vec();

    for group in &mut groups {
        let Some(original) = group.as_mapping() else {
            continue;
        };
        let providers = source_providers(original, &config);
        if text(group, "type") != Some("load-balance") {
            unmanaged_providers.extend(providers);
            continue;
        }
        let name = text(group, "name").unwrap_or_default().to_owned();
        if let Some(reason) = unsupported_reason(original, &providers, &proxy_types, &group_names) {
            unmanaged_providers.extend(providers);
            plan.unsupported.push(UnsupportedGroup { name, reason });
            continue;
        }
        let original = original.clone();
        let (test_url, expected_status) = test_settings(&original, &providers, &config);
        let mut adaptive = original.clone();
        adaptive.insert("adaptive".into(), true.into());
        adaptive.insert("manual-health-check".into(), true.into());
        adaptive.insert("url".into(), test_url.into());
        if !adaptive.contains_key("expected-status") {
            if let Some(expected_status) = expected_status {
                adaptive.insert("expected-status".into(), expected_status.into());
            }
        }
        *group = adaptive.into();
        managed_providers.extend(providers);
        plan.groups.push(GroupPlan { name, original });
    }

    if !plan.groups.is_empty() {
        if let Some(providers) = config.get_mut("proxy-providers").and_then(Value::as_mapping_mut) {
            for name in managed_providers.difference(&unmanaged_providers) {
                if let Some(provider) = providers.get_mut(name.as_str()).and_then(Value::as_mapping_mut) {
                    let health = provider
                        .entry(Value::from("health-check"))
                        .or_insert_with(|| Mapping::new().into());
                    let health = health
                        .as_mapping_mut()
                        .context("participating provider health-check must be a mapping")?;
                    health.insert("enable".into(), false.into());
                }
            }
        }
        config.insert("proxy-groups".into(), groups.into());
    }
    Ok((config, Some(plan)))
}

fn unsupported_reason(
    original: &Mapping,
    providers: &[String],
    proxy_types: &HashMap<&str, &str>,
    group_names: &HashSet<&str>,
) -> Option<String> {
    let reason = if original.get("name").and_then(Value::as_str).is_none_or(str::is_empty) {
        "The load-balance group has no usable name"
    } else if original.get("strategy").and_then(Value::as_str) != Some("round-robin") {
        "Adaptive routing only supports an explicitly configured round-robin strategy"
    } else if original
        .get("exclude-filter")
        .is_some_and(|value| value.as_str() != Some(""))
    {
        "Adaptive routing does not support exclude-filter in this version"
    } else if original
        .get("exclude-type")
        .and_then(Value::as_str)
        .is_some_and(|types| types.split('|').any(|kind| kind.eq_ignore_ascii_case("reject")))
    {
        "Adaptive routing does not support excluding Reject candidates in this version"
    } else if providers.len() > 1 {
        "Multiple proxy providers may contain ambiguous same-name candidates"
    } else if original.get("proxies").is_some_and(|value| {
        value.as_sequence().is_none_or(|proxies| {
            proxies.iter().any(|proxy| {
                proxy.as_str().is_none_or(|name| {
                    BUILTINS.contains(&name)
                        || group_names.contains(name)
                        || !proxy_types.contains_key(name)
                        || proxy_types.get(name).is_some_and(|kind| is_special_type(kind))
                })
            })
        })
    }) {
        "Special, nested, or unresolved explicit candidates cannot be safely enhanced"
    } else if (flag(original, "include-all") || flag(original, "include-all-proxies"))
        && proxy_types.values().any(|kind| is_special_type(kind))
    {
        "Include-all contains special proxy types that cannot be safely ranked"
    } else {
        return None;
    };
    Some(reason.to_owned())
}

fn is_special_type(kind: &str) -> bool {
    [
        "direct",
        "reject",
        "reject-drop",
        "pass",
        "compatible",
        "wireguard",
        "tailscale",
    ]
    .iter()
    .any(|special| kind.eq_ignore_ascii_case(special))
}

fn source_providers(original: &Mapping, config: &Mapping) -> Vec<String> {
    let mut providers = if flag(original, "include-all") || flag(original, "include-all-providers") {
        let mut names: Vec<_> = config
            .get("proxy-providers")
            .and_then(Value::as_mapping)
            .into_iter()
            .flat_map(|providers| providers.keys().filter_map(Value::as_str).map(str::to_owned))
            .collect();
        names.sort();
        names
    } else {
        original
            .get("use")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    providers.dedup();
    providers
}

fn test_settings(original: &Mapping, providers: &[String], config: &Mapping) -> (String, Option<String>) {
    let group_url = original
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty());
    let provider_health = if group_url.is_none() {
        providers.iter().find_map(|name| {
            let health = config.get("proxy-providers")?.get(name)?.get("health-check")?;
            health.get("url")?.as_str().filter(|url| !url.is_empty())?;
            Some(health)
        })
    } else {
        None
    };
    let url = group_url
        .or_else(|| provider_health?.get("url")?.as_str())
        .unwrap_or(DEFAULT_TEST_URL)
        .to_owned();
    let expected_status = original
        .get("expected-status")
        .or_else(|| provider_health?.get("expected-status"))
        .and_then(|status| {
            status.as_str().map(str::to_owned).or_else(|| {
                status
                    .as_i64()
                    .map(|status| status.to_string())
                    .or_else(|| status.as_u64().map(|status| status.to_string()))
            })
        });
    (url, expected_status)
}

fn flag(mapping: &Mapping, key: &str) -> bool {
    mapping.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn sequence<'a>(config: &'a Mapping, key: &str) -> &'a [Value] {
    config
        .get(key)
        .and_then(Value::as_sequence)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn config(yaml: &str) -> Mapping {
        serde_yaml_ng::from_str(yaml).expect("valid test config")
    }

    const STATIC: &str = "proxies: [{name: fast, type: ss}, {name: slow, type: ss}]\nproxy-groups: [{name: LB, type: load-balance, strategy: round-robin, proxies: [fast, slow], url: 'https://test.invalid/204', interval: 600}]";

    fn group<'a>(config: &'a Mapping, name: &str) -> &'a Value {
        sequence(config, "proxy-groups")
            .iter()
            .find(|group| text(group, "name") == Some(name))
            .expect("group exists")
    }

    #[test]
    fn disabled_keeps_yaml_exactly_and_publishes_no_plan() {
        let original = config(STATIC);
        let (runtime, plan) = transform(original.clone(), false).expect("disabled");
        assert_eq!(runtime, original);
        assert!(plan.is_none());
    }

    #[test]
    fn adaptive_keeps_original_group_members_strategy_and_options_without_internal_groups() {
        let original = config(&STATIC.replace("interval: 600", "hash-key: in-user, interval: 600"));
        let (runtime, plan) = transform(original.clone(), true).expect("transform");
        let plan = plan.expect("plan");
        let parent = group(&runtime, "LB");
        let mut expected = group(&original, "LB").as_mapping().expect("original").clone();
        expected.insert("adaptive".into(), true.into());
        expected.insert("manual-health-check".into(), true.into());
        assert_eq!(parent.as_mapping(), Some(&expected));
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(
            plan.groups[0].original,
            *group(&original, "LB").as_mapping().expect("original")
        );
        assert!(plan.internal_names.is_empty());
        assert_eq!(
            sequence(&runtime, "proxy-groups").len(),
            sequence(&original, "proxy-groups").len()
        );
    }

    #[test]
    fn provider_and_include_all_sources_and_filters_stay_in_original_group() {
        let original = config(
            "proxy-providers: {only: {type: http, health-check: {url: 'https://provider.invalid/204'}}}\nproxies: [{name: local, type: ss}]\nproxy-groups: [{name: LB, type: load-balance, strategy: round-robin, include-all: true, filter: 'HK`JP', exclude-type: vmess}]",
        );
        let (runtime, plan) = transform(original.clone(), true).expect("transform");
        for key in [
            "proxies",
            "use",
            "include-all",
            "include-all-proxies",
            "include-all-providers",
            "filter",
            "exclude-filter",
            "exclude-type",
        ] {
            assert_eq!(group(&runtime, "LB").get(key), group(&original, "LB").get(key));
        }
        assert_eq!(group(&runtime, "LB")["url"], "https://provider.invalid/204");
        assert!(plan.expect("plan").internal_names.is_empty());
    }

    #[test]
    fn shared_providers_keep_health_checks_for_unmanaged_groups() {
        for consumer in [
            "{name: ordinary, type: url-test, use: [only]}",
            "{name: ordinary, type: fallback, include-all-providers: true}",
            "{name: ordinary, type: load-balance, strategy: consistent-hashing, use: [only]}",
        ] {
            let original = config(&format!(
                "proxy-providers: {{only: {{type: http, health-check: {{enable: true, interval: 60, url: 'https://provider.invalid/204'}}}}}}\nproxy-groups: [{{name: LB, type: load-balance, strategy: round-robin, use: [only]}}, {consumer}]"
            ));
            let (runtime, plan) = transform(original.clone(), true).expect("transform");
            assert_eq!(runtime["proxy-providers"], original["proxy-providers"]);
            assert_eq!(group(&runtime, "ordinary"), group(&original, "ordinary"));
            assert_eq!(plan.expect("plan").groups.len(), 1);
        }
    }

    #[test]
    fn expected_status_keeps_numeric_codes_and_string_ranges_for_native_health_checks() {
        for yaml_status in ["204", "'200/302'", "'200-299'"] {
            let original = config(&STATIC.replace(
                "interval: 600",
                &format!("expected-status: {yaml_status}, interval: 600"),
            ));
            let (runtime, _) = transform(original.clone(), true).expect("transform");
            assert_eq!(
                group(&runtime, "LB")["expected-status"],
                group(&original, "LB")["expected-status"]
            );
        }
    }

    #[test]
    fn expected_status_inherits_only_with_provider_test_url_and_group_status_wins() {
        for (overrides, expected_url, expected_status) in [
            ("", "https://provider.invalid/204", Some(Value::from("204"))),
            (
                ", expected-status: '200/302'",
                "https://provider.invalid/204",
                Some(Value::from("200/302")),
            ),
            (", url: 'https://group.invalid/204'", "https://group.invalid/204", None),
            (
                ", url: 'https://group.invalid/204', expected-status: 302",
                "https://group.invalid/204",
                Some(Value::from(302)),
            ),
        ] {
            let original = config(&format!(
                "proxy-providers: {{only: {{type: http, health-check: {{url: 'https://provider.invalid/204', expected-status: 204}}}}}}\nproxy-groups: [{{name: LB, type: load-balance, strategy: round-robin, use: [only]{overrides}}}]"
            ));
            let (runtime, _) = transform(original, true).expect("transform");
            assert_eq!(group(&runtime, "LB")["url"], expected_url);
            assert_eq!(group(&runtime, "LB").get("expected-status"), expected_status.as_ref());
        }
    }

    #[test]
    fn only_participating_provider_health_checks_are_disabled_in_runtime() {
        let original = config(
            "proxy-providers: {participating: {type: http, health-check: {enable: true, interval: 60, url: 'https://provider.invalid/204'}}, other: {type: http, health-check: {enable: true, interval: 10}}}\nproxy-groups: [{name: LB, type: load-balance, strategy: round-robin, use: [participating]}, {name: ordinary, type: url-test, use: [other]}]",
        );
        let (runtime, _) = transform(original.clone(), true).expect("transform");
        assert_eq!(
            runtime["proxy-providers"]["participating"]["health-check"]["enable"],
            false
        );
        assert_eq!(
            runtime["proxy-providers"]["participating"]["health-check"]["interval"],
            60
        );
        assert_eq!(
            runtime["proxy-providers"]["other"],
            original["proxy-providers"]["other"]
        );
        assert_eq!(group(&runtime, "ordinary"), group(&original, "ordinary"));
        let (disabled, plan) = transform(original.clone(), false).expect("disabled");
        assert_eq!(disabled, original);
        assert!(plan.is_none());
    }

    #[test]
    fn unsupported_strategies_keep_entire_source_mapping_unchanged() {
        for strategy in ["", ", strategy: sticky-sessions", ", strategy: consistent-hashing"] {
            let original = config(&format!(
                "proxies: [{{name: fast, type: ss}}]\nproxy-groups: [{{name: LB, type: load-balance, proxies: [fast], hash-key: in-user{strategy}}}]"
            ));
            let (runtime, plan) = transform(original.clone(), true).expect("transform");
            let plan = plan.expect("plan");
            assert_eq!(runtime, original);
            assert!(plan.groups.is_empty());
            assert_eq!(plan.unsupported.len(), 1);
            assert!(plan.unsupported[0].reason.contains("round-robin"));
        }
    }

    #[test]
    fn unsafe_sources_are_reported_without_changing_the_group() {
        for body in [
            "proxies: [DIRECT]",
            "proxies: [fast], exclude-filter: REJECT",
            "proxies: [fast], exclude-type: Reject",
            "use: [one, two]",
            "proxies: [nested]",
        ] {
            let original = config(&format!(
                "proxies: [{{name: fast, type: ss}}]\nproxy-providers: {{one: {{}}, two: {{}}}}\nproxy-groups: [{{name: nested, type: select, proxies: [fast]}}, {{name: LB, type: load-balance, strategy: round-robin, {body}}}]"
            ));
            let (runtime, plan) = transform(original.clone(), true).expect("transform");
            let plan = plan.expect("plan");
            assert_eq!(runtime, original);
            assert!(plan.groups.is_empty());
            assert_eq!(plan.unsupported.len(), 1);
            assert!(!plan.unsupported[0].reason.is_empty());
            assert!(plan.internal_names.is_empty());
        }
    }

    #[test]
    fn existing_listeners_are_unchanged_without_additional_probe_ports() {
        let mut original = config(STATIC);
        original.insert(
            "listeners".into(),
            config("a: [{name: user, type: http, listen: '127.0.0.1', port: '19000-19010', proxy: LB}]")["a"].clone(),
        );
        let (runtime, plan) = transform(original.clone(), true).expect("transform");
        assert_eq!(runtime["listeners"], original["listeners"]);
        assert!(plan.expect("plan").internal_names.is_empty());
    }
}
