use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri_plugin_mihomo::models::Proxies;

use crate::enhance::load_balance::EnhancementPlan;

const NATIVE_CAPABILITY: &str = "verge-adaptive1";

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CandidateStatus {
    pub name: String,
    pub score: Option<f64>,
    pub selected: bool,
    pub reachable: bool,
    pub fresh_health: bool,
    pub proven: bool,
    pub confidence: f64,
    pub mean_rate: f64,
    pub peak_rate: f64,
    pub latency_ms: Option<f64>,
    pub state: String,
    pub cooldown_remaining_seconds: Option<u32>,
    pub last_checked_seconds_ago: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GroupStatus {
    pub state: String,
    pub reason: Option<String>,
    pub candidates: Vec<CandidateStatus>,
}

impl GroupStatus {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            state: "unsupported".into(),
            reason: Some(reason.into()),
            candidates: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeStatusEnvelope {
    schema_version: u32,
    capability: String,
    #[serde(flatten)]
    status: GroupStatus,
}

pub fn native_statuses(proxies: &Proxies, plan: &EnhancementPlan) -> BTreeMap<String, GroupStatus> {
    plan.groups
        .iter()
        .map(|group| {
            let proxy = proxies.proxies.get(&group.name);
            let mut status = parse_native_status(proxy.and_then(|proxy| proxy.unknown_fields.get("adaptive")));
            if status.candidates.iter().any(|candidate| {
                !proxy
                    .and_then(|proxy| proxy.all.as_ref())
                    .is_some_and(|members| members.contains(&candidate.name))
            }) {
                status =
                    GroupStatus::unsupported("The native adaptive status does not match the group's current members");
            }
            (group.name.clone(), status)
        })
        .collect()
}

fn parse_native_status(payload: Option<&Value>) -> GroupStatus {
    let Some(payload) = payload else {
        return GroupStatus::unsupported(
            "Native adaptive status is unavailable; routing requires the bundled v1.19.32-verge-adaptive1 core",
        );
    };
    let Ok(envelope) = serde_json::from_value::<NativeStatusEnvelope>(payload.clone()) else {
        return GroupStatus::unsupported("The core returned malformed native adaptive status");
    };
    if envelope.schema_version != 1 || envelope.capability != NATIVE_CAPABILITY {
        return GroupStatus::unsupported(
            "The core's native adaptive capability is incompatible; expected verge-adaptive1 schema 1",
        );
    }
    let mut status = envelope.status;
    let mut names = HashSet::new();
    if !matches!(
        status.state.as_str(),
        "warming" | "active" | "trial" | "draining" | "degraded" | "unavailable" | "unsupported"
    ) || status.candidates.iter().filter(|candidate| candidate.selected).count() > 3
        || status.candidates.iter().any(|candidate| {
            candidate.name.is_empty()
                || !names.insert(&candidate.name)
                || !matches!(
                    candidate.state.as_str(),
                    "selected" | "trial" | "cooldown" | "unavailable" | "probing" | "untested" | "standby"
                )
        })
    {
        return GroupStatus::unsupported("The core returned invalid native adaptive status fields");
    }
    let mut metrics_omitted = false;
    for candidate in &mut status.candidates {
        let invalid_score = candidate
            .score
            .is_some_and(|score| !score.is_finite() || !(0.0..=100.0).contains(&score));
        let invalid_confidence = !candidate.confidence.is_finite() || !(0.0..=1.0).contains(&candidate.confidence);
        let invalid_mean = !candidate.mean_rate.is_finite() || candidate.mean_rate < 0.0;
        let invalid_peak = !candidate.peak_rate.is_finite() || candidate.peak_rate < 0.0;
        let invalid_latency = candidate
            .latency_ms
            .is_some_and(|latency| !latency.is_finite() || latency < 0.0);
        if invalid_score {
            candidate.score = None;
        }
        if invalid_confidence || invalid_mean || invalid_peak {
            candidate.score = None;
            candidate.proven = false;
        }
        if invalid_confidence {
            candidate.confidence = 0.0;
        }
        if invalid_mean {
            candidate.mean_rate = 0.0;
        }
        if invalid_peak {
            candidate.peak_rate = 0.0;
        }
        if invalid_latency {
            candidate.latency_ms = None;
        }
        metrics_omitted |= invalid_score || invalid_confidence || invalid_mean || invalid_peak || invalid_latency;
    }
    if metrics_omitted {
        let detail = "Some native display metrics were omitted because they were outside the supported range; routing membership is unchanged";
        status.reason = Some(match status.reason {
            Some(reason) => format!("{reason}; {detail}"),
            None => detail.into(),
        });
    }
    if status.state == "degraded" {
        status.state = if status.candidates.iter().any(|candidate| candidate.selected) {
            "active"
        } else {
            "unavailable"
        }
        .into();
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> Value {
        json!({
            "schemaVersion": 1,
            "capability": "verge-adaptive1",
            "state": "active",
            "reason": null,
            "candidates": [{
                "name": "fast",
                "score": null,
                "selected": true,
                "reachable": true,
                "freshHealth": true,
                "proven": false,
                "confidence": 0.0,
                "meanRate": 0.0,
                "peakRate": 0.0,
                "latencyMs": 0.0,
                "state": "selected",
                "cooldownRemainingSeconds": null,
                "lastCheckedSecondsAgo": 0
            }]
        })
    }

    #[test]
    fn plugin_proxies_preserve_native_envelope_and_existing_camel_case_contract() -> anyhow::Result<()> {
        let native = payload();
        let proxies: Proxies = serde_json::from_value(json!({"proxies": {
            "LB": {"name": "LB", "type": "LoadBalance", "all": ["fast"], "adaptive": native},
            "fast": {"name": "fast", "type": "Shadowsocks"}
        }}))?;
        let source = serde_yaml_ng::from_str(
            "proxies: [{name: fast, type: ss}]\nproxy-groups: [{name: LB, type: load-balance, strategy: round-robin, proxies: [fast]}]",
        )?;
        let (_, plan) = crate::enhance::load_balance::transform(source, true)?;
        let plan = plan.ok_or_else(|| anyhow::anyhow!("expected plan"))?;
        let statuses = native_statuses(&proxies, &plan);
        let status = statuses.get("LB").ok_or_else(|| anyhow::anyhow!("expected status"))?;
        assert_eq!(status.state, "active");
        assert_eq!(status.candidates[0].latency_ms, Some(0.0));
        assert_eq!(status.candidates[0].score, None);
        assert!(status.candidates[0].selected);
        let mut expected = native;
        let expected = expected
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("expected object"))?;
        expected.remove("schemaVersion");
        expected.remove("capability");
        assert_eq!(serde_json::to_value(status)?, json!(expected));
        Ok(())
    }

    #[test]
    fn missing_malformed_and_incompatible_payloads_are_explicitly_unsupported() {
        assert_eq!(parse_native_status(None).state, "unsupported");
        for invalid in [
            Value::Null,
            json!(true),
            json!({}),
            json!({"schemaVersion": 1, "capability": "verge-adaptive1", "state": "active"}),
        ] {
            let status = parse_native_status(Some(&invalid));
            assert_eq!(status.state, "unsupported");
            assert!(status.reason.is_some_and(|reason| !reason.is_empty()));
            assert!(status.candidates.is_empty());
        }
        for (key, value) in [
            ("schemaVersion", json!(2)),
            ("capability", json!("verge-trial1")),
            ("state", json!("warming-up")),
        ] {
            let mut invalid = payload();
            invalid[key] = value;
            assert_eq!(parse_native_status(Some(&invalid)).state, "unsupported");
        }
        for (key, value) in [("lastCheckedSecondsAgo", json!(-1)), ("selected", json!("true"))] {
            let mut invalid = payload();
            invalid["candidates"][0][key] = value;
            assert_eq!(parse_native_status(Some(&invalid)).state, "unsupported");
        }
    }

    #[test]
    fn invalid_display_metrics_do_not_hide_valid_native_membership() {
        for (key, value) in [
            ("score", json!(1000)),
            ("score", json!(-1)),
            ("confidence", json!(1.1)),
            ("meanRate", json!(-1)),
            ("peakRate", json!(-1)),
            ("latencyMs", json!(-1)),
        ] {
            let mut native = payload();
            native["candidates"][0][key] = value;
            let status = parse_native_status(Some(&native));
            assert_eq!(status.state, "active", "{key}");
            assert_eq!(status.candidates.len(), 1);
            assert!(status.candidates[0].selected);
            assert!(status.candidates[0].reachable);
            assert!(
                status
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("display metrics"))
            );
            if key == "score" {
                assert_eq!(status.candidates[0].score, None);
            }
            if key == "latencyMs" {
                assert_eq!(status.candidates[0].latency_ms, None);
            }
        }
    }

    #[test]
    fn stale_candidate_with_overflowing_score_does_not_reject_working_pool() {
        let mut native = payload();
        let mut stale = native["candidates"][0].clone();
        stale["name"] = json!("stale");
        stale["score"] = json!(1000);
        stale["selected"] = json!(false);
        stale["reachable"] = json!(false);
        stale["freshHealth"] = json!(false);
        stale["state"] = json!("unavailable");
        native["candidates"].as_array_mut().unwrap().push(stale);
        let status = parse_native_status(Some(&native));
        assert_eq!(status.state, "active");
        assert!(status.candidates[0].selected);
        assert_eq!(status.candidates[1].score, None);
        assert!(!status.candidates[1].selected);
        assert!(!status.candidates[1].reachable);
    }

    #[test]
    fn unsafe_membership_is_still_rejected_independently_of_display_metrics() {
        let mut duplicate = payload();
        let candidate = duplicate["candidates"][0].clone();
        duplicate["candidates"].as_array_mut().unwrap().push(candidate);
        assert_eq!(parse_native_status(Some(&duplicate)).state, "unsupported");
        let mut excessive = payload();
        for name in ["second", "third", "fourth"] {
            let mut candidate = excessive["candidates"][0].clone();
            candidate["name"] = json!(name);
            excessive["candidates"].as_array_mut().unwrap().push(candidate);
        }
        assert_eq!(parse_native_status(Some(&excessive)).state, "unsupported");
    }

    #[test]
    fn reduced_pool_status_uses_existing_ui_states_without_claiming_warming() {
        let mut native = payload();
        native["state"] = json!("degraded");
        assert_eq!(parse_native_status(Some(&native)).state, "active");
        native["candidates"][0]["selected"] = json!(false);
        assert_eq!(parse_native_status(Some(&native)).state, "unavailable");
        native["state"] = json!("unavailable");
        assert_eq!(parse_native_status(Some(&native)).state, "unavailable");
    }

    #[test]
    fn compatible_extra_fields_are_ignored_without_inventing_health() {
        let mut native = payload();
        native["selected"] = json!(["fast"]);
        native["trial"] = Value::Null;
        native["candidates"][0]["latencyMs"] = Value::Null;
        native["candidates"][0]["reachable"] = json!(false);
        native["candidates"][0]["freshHealth"] = json!(false);
        native["candidates"][0]["selected"] = json!(false);
        native["candidates"][0]["state"] = json!("untested");
        native["state"] = json!("warming");
        let status = parse_native_status(Some(&native));
        assert_eq!(status.state, "warming");
        assert!(!status.candidates[0].reachable);
        assert!(!status.candidates[0].selected);
        assert_eq!(status.candidates[0].latency_ms, None);
    }
}
