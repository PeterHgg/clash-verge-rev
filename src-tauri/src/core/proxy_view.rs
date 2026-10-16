use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{core::load_balance::GroupStatus, enhance::load_balance::EnhancementPlan};

use serde::{Serialize, Serializer, ser::SerializeMap as _};
use tauri_plugin_mihomo::models::{
    DelayHistory, Extra, Proxies, Proxy, ProxyProvider, ProxyProviders, ProxyType, VehicleType,
};

pub struct ProxyViewInput {
    pub runtime_group_order: Vec<String>,
    pub group_scopes: BTreeMap<String, GroupScope>,
    pub proxies: Proxies,
    pub providers: Option<ProxyProviders>,
}

/// The providers a group draws from (Mihomo order) and its `exclude-type` entries.
pub struct GroupScope {
    pub providers: Vec<String>,
    pub exclude_types: Vec<String>,
}

pub struct ProxyViewBuilder;

#[derive(Debug, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxyViewV1 {
    pub schema_version: u8,
    pub order_source: ProxyViewOrderSource,
    pub provider_state: ProxyViewProviderState,
    pub global: Option<ProxyGroupView>,
    pub direct: Option<String>,
    pub groups: Vec<ProxyGroupView>,
    pub records: BTreeMap<String, ProxyNodeView>,
    pub standalone: Vec<String>,
    pub providers: Vec<ProxyProviderView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ProxyViewOrderSource {
    Runtime,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ProxyViewProviderState {
    Ready,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
pub struct ProxyCapabilities {
    pub udp: bool,
    pub xudp: bool,
    pub tfo: bool,
    pub mptcp: bool,
    pub smux: bool,
}

#[derive(Serialize, specta::Type)]
struct ProxyLatencyHistory<'a> {
    #[specta(type = Vec<crate::ipc::DelayHistory>)]
    history: &'a [DelayHistory],
}

fn serialize_proxy_latency_history<S: Serializer>(
    extra: &HashMap<String, Extra>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(extra.len()))?;
    for (url, extra) in extra {
        map.serialize_entry(
            url,
            &ProxyLatencyHistory {
                history: &extra.history,
            },
        )?;
    }
    map.end()
}

#[derive(Debug, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGroupView {
    pub name: String,
    #[serde(rename = "type")]
    #[specta(type = String)]
    pub proxy_type: ProxyType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_url: Option<String>,
    #[specta(type = Vec<crate::ipc::DelayHistory>)]
    pub history: Vec<DelayHistory>,
    #[serde(serialize_with = "serialize_proxy_latency_history")]
    #[specta(type = HashMap<String, ProxyLatencyHistory<'static>>)]
    pub extra: HashMap<String, Extra>,
    #[serde(flatten)]
    pub capabilities: ProxyCapabilities,
    pub members: Vec<ProxyMemberRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhancement: Option<GroupStatus>,
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxyNodeView {
    pub record_id: String,
    pub name: String,
    #[serde(rename = "type")]
    #[specta(type = String)]
    pub proxy_type: ProxyType,
    #[specta(type = Vec<crate::ipc::DelayHistory>)]
    pub history: Vec<DelayHistory>,
    #[serde(serialize_with = "serialize_proxy_latency_history")]
    #[specta(type = HashMap<String, ProxyLatencyHistory<'static>>)]
    pub extra: HashMap<String, Extra>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_url: Option<String>,
    #[serde(flatten)]
    pub capabilities: ProxyCapabilities,
    pub source: ProxyNodeSource,
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProxyNodeSource {
    Core {
        #[serde(rename = "proxyName")]
        proxy_name: String,
    },
    Provider {
        #[serde(rename = "providerName")]
        provider_name: String,
        #[serde(rename = "proxyName")]
        proxy_name: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProxyMemberRef {
    Group {
        name: String,
    },
    Node {
        name: String,
        #[serde(rename = "recordId")]
        record_id: String,
    },
    Unresolved {
        name: String,
        reason: ProxyMemberUnresolvedReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub enum ProxyMemberUnresolvedReason {
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "ambiguous")]
    Ambiguous,
    #[serde(rename = "provider-unavailable")]
    ProviderUnavailable,
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProviderView {
    pub name: String,
    pub vehicle_type: ProxyProviderVehicleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_info: Option<ProxySubscriptionInfo>,
    pub proxy_record_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub enum ProxyProviderVehicleType {
    #[serde(rename = "HTTP")]
    Http,
    #[serde(rename = "File")]
    File,
    #[serde(rename = "Inline")]
    Inline,
}

#[derive(Debug, PartialEq, Eq, Serialize, specta::Type)]
pub struct ProxySubscriptionInfo {
    pub upload: i64,
    pub download: i64,
    pub total: i64,
    pub expire: i64,
}

/// Proxy name -> `(provider name, record id)` in provider order.
type ProviderCandidates = BTreeMap<String, Vec<(String, String)>>;

struct MemberResolver<'a> {
    group_names: BTreeSet<String>,
    group_scopes: &'a BTreeMap<String, GroupScope>,
    records: &'a BTreeMap<String, ProxyNodeView>,
    core_node_ids: &'a BTreeMap<String, String>,
    provider_candidates: &'a ProviderCandidates,
    provider_available: bool,
}

impl<'a> MemberResolver<'a> {
    /// Same-name records in the order Mihomo lists them in `group`: `use` order, after `exclude-type`.
    fn scoped_candidates(&self, group: &str, name: &str) -> Vec<&'a String> {
        let (Some(scope), Some(candidates)) = (self.group_scopes.get(group), self.provider_candidates.get(name)) else {
            return Vec::new();
        };
        scope
            .providers
            .iter()
            .flat_map(|provider| candidates.iter().filter(move |(owner, _)| owner == provider))
            .map(|(_, record_id)| record_id)
            .filter(|record_id| {
                !self.records.get(*record_id).is_some_and(|node| {
                    scope
                        .exclude_types
                        .iter()
                        .any(|excluded| excluded.eq_ignore_ascii_case(node.proxy_type.as_str()))
                })
            })
            .collect()
    }

    fn resolve(&self, name: String, scoped: Option<&String>) -> ProxyMemberRef {
        if self.group_names.contains(&name) {
            ProxyMemberRef::Group { name }
        } else if let Some(record_id) = self.core_node_ids.get(&name) {
            ProxyMemberRef::Node {
                name,
                record_id: record_id.clone(),
            }
        } else if !self.provider_available {
            ProxyMemberRef::Unresolved {
                name,
                reason: ProxyMemberUnresolvedReason::ProviderUnavailable,
            }
        } else {
            let candidates = self.provider_candidates.get(&name).map_or(&[][..], Vec::as_slice);
            match (scoped, candidates) {
                (Some(record_id), _) | (None, [(_, record_id)]) => ProxyMemberRef::Node {
                    name,
                    record_id: record_id.clone(),
                },
                (None, []) => ProxyMemberRef::Unresolved {
                    name,
                    reason: ProxyMemberUnresolvedReason::Missing,
                },
                (None, _) => ProxyMemberRef::Unresolved {
                    name,
                    reason: ProxyMemberUnresolvedReason::Ambiguous,
                },
            }
        }
    }
}

impl ProxyViewBuilder {
    pub fn build_enhanced(
        input: ProxyViewInput,
        plan: Option<&EnhancementPlan>,
        mut status: BTreeMap<String, GroupStatus>,
    ) -> ProxyViewV1 {
        let Some(plan) = plan else {
            return Self::build(input);
        };
        let mut view = Self::build(input);
        for group in &mut view.groups {
            if plan.groups.iter().any(|planned| planned.name == group.name) {
                let enhancement = status
                    .remove(&group.name)
                    .unwrap_or_else(|| GroupStatus::unsupported("Native adaptive status is unavailable"));
                let scores: BTreeMap<_, _> = enhancement
                    .candidates
                    .iter()
                    .filter_map(|candidate| {
                        candidate
                            .score
                            .filter(|score| score.is_finite())
                            .map(|score| (candidate.name.as_str(), score))
                    })
                    .collect();
                if !scores.is_empty() {
                    group.members.sort_by(|left, right| {
                        let left = member_name(left);
                        let right = member_name(right);
                        match (scores.get(left), scores.get(right)) {
                            (Some(a), Some(b)) => b.total_cmp(a).then_with(|| left.cmp(right)),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => left.cmp(right),
                        }
                    });
                }
                group.enhancement = Some(enhancement);
            } else if let Some(unsupported) = plan.unsupported.iter().find(|planned| planned.name == group.name) {
                group.enhancement = Some(GroupStatus {
                    state: "unsupported".into(),
                    reason: Some(unsupported.reason.clone()),
                    candidates: Vec::new(),
                });
            }
        }
        view
    }

    pub fn build(input: ProxyViewInput) -> ProxyViewV1 {
        let ProxyViewInput {
            runtime_group_order,
            group_scopes,
            proxies,
            providers,
        } = input;
        let provider_state = if providers.is_some() {
            ProxyViewProviderState::Ready
        } else {
            ProxyViewProviderState::Unavailable
        };
        let (mut core_groups, core_nodes) = partition_core(proxies, providers.is_some());
        let (mut records, core_node_ids) = build_core_records(core_nodes);
        let (providers, provider_candidates) = build_provider_records(providers, &mut records);
        let resolver = MemberResolver {
            group_names: core_groups.keys().cloned().collect(),
            group_scopes: &group_scopes,
            records: &records,
            core_node_ids: &core_node_ids,
            provider_candidates: &provider_candidates,
            provider_available: provider_state == ProxyViewProviderState::Ready,
        };

        let global = core_groups
            .remove("GLOBAL")
            .map(|proxy| build_group("GLOBAL".to_owned(), proxy, &resolver));
        let (groups, order_source) = build_ordered_groups(core_groups, runtime_group_order, &resolver);
        let direct = core_node_ids.get("DIRECT").cloned();
        let standalone = build_standalone(&core_node_ids);

        ProxyViewV1 {
            schema_version: 1,
            order_source,
            provider_state,
            global,
            direct,
            groups,
            records,
            standalone,
            providers,
        }
    }
}

fn partition_core(proxies: Proxies, providers_available: bool) -> (BTreeMap<String, Proxy>, BTreeMap<String, Proxy>) {
    let mut groups = BTreeMap::new();
    let mut nodes = BTreeMap::new();

    for (name, proxy) in proxies.proxies {
        if proxy.all.is_some() {
            groups.insert(name, proxy);
        } else if !providers_available || proxy.provider_name.is_empty() {
            nodes.insert(name, proxy);
        }
    }

    (groups, nodes)
}

fn build_core_records(
    core_nodes: BTreeMap<String, Proxy>,
) -> (BTreeMap<String, ProxyNodeView>, BTreeMap<String, String>) {
    let mut records = BTreeMap::new();
    let mut ids = BTreeMap::new();

    for (index, (name, proxy)) in core_nodes.into_iter().enumerate() {
        let record_id = format!("c:{index}");
        ids.insert(name.clone(), record_id.clone());
        records.insert(
            record_id.clone(),
            build_node(
                record_id,
                name.clone(),
                proxy,
                ProxyNodeSource::Core { proxy_name: name },
            ),
        );
    }

    (records, ids)
}

fn build_provider_records(
    providers: Option<ProxyProviders>,
    records: &mut BTreeMap<String, ProxyNodeView>,
) -> (Vec<ProxyProviderView>, ProviderCandidates) {
    let providers = providers
        .map(|providers| providers.providers.into_iter().collect::<BTreeMap<_, _>>())
        .unwrap_or_default();
    let mut views = Vec::new();
    let mut candidates = BTreeMap::new();

    for (provider_name, provider) in providers {
        let ProxyProvider {
            vehicle_type,
            proxies,
            updated_at,
            subscription_info,
            ..
        } = provider;
        let vehicle_type = match vehicle_type {
            VehicleType::HTTP => ProxyProviderVehicleType::Http,
            VehicleType::File => ProxyProviderVehicleType::File,
            VehicleType::Inline => ProxyProviderVehicleType::Inline,
            _ => continue,
        };
        let provider_index = views.len();
        let mut proxy_record_ids = Vec::new();

        for (member_index, proxy) in proxies.into_iter().enumerate() {
            let record_id = format!("p:{provider_index}:{member_index}");
            let proxy_name = proxy.name.clone();
            let node_name = proxy.name.clone();
            candidates
                .entry(proxy_name.clone())
                .or_insert_with(Vec::new)
                .push((provider_name.clone(), record_id.clone()));
            records.insert(
                record_id.clone(),
                build_node(
                    record_id.clone(),
                    node_name,
                    proxy,
                    ProxyNodeSource::Provider {
                        provider_name: provider_name.clone(),
                        proxy_name,
                    },
                ),
            );
            proxy_record_ids.push(record_id);
        }

        views.push(ProxyProviderView {
            name: provider_name,
            vehicle_type,
            updated_at,
            subscription_info: subscription_info.map(|subscription_info| ProxySubscriptionInfo {
                upload: subscription_info.upload,
                download: subscription_info.download,
                total: subscription_info.total,
                expire: subscription_info.expire,
            }),
            proxy_record_ids,
        });
    }

    (views, candidates)
}

fn build_group(name: String, proxy: Proxy, resolver: &MemberResolver<'_>) -> ProxyGroupView {
    let Proxy {
        all,
        fixed,
        hidden,
        icon,
        now,
        test_url,
        history,
        extra,
        udp,
        xudp,
        tfo,
        mptcp,
        smux,
        proxy_type,
        ..
    } = proxy;
    // The n-th occurrence of a name in `all` is the n-th scoped candidate.
    let mut seen = BTreeMap::<String, (usize, Vec<&String>)>::new();
    let members = all
        .unwrap_or_default()
        .into_iter()
        .map(|member| {
            let (occurrence, scoped) = seen
                .entry(member.clone())
                .or_insert_with(|| (0, resolver.scoped_candidates(&name, &member)));
            let picked = scoped.get(*occurrence).or_else(|| scoped.first()).copied();
            *occurrence += 1;
            resolver.resolve(member, picked)
        })
        .collect();

    ProxyGroupView {
        name,
        proxy_type,
        now,
        fixed,
        hidden,
        icon,
        test_url,
        history,
        extra,
        capabilities: ProxyCapabilities {
            udp,
            xudp,
            tfo,
            mptcp,
            smux,
        },
        members,
        enhancement: None,
    }
}

fn member_name(member: &ProxyMemberRef) -> &str {
    match member {
        ProxyMemberRef::Group { name }
        | ProxyMemberRef::Node { name, .. }
        | ProxyMemberRef::Unresolved { name, .. } => name,
    }
}

fn build_node(record_id: String, name: String, proxy: Proxy, source: ProxyNodeSource) -> ProxyNodeView {
    let Proxy {
        id,
        hidden,
        icon,
        test_url,
        history,
        extra,
        udp,
        xudp,
        tfo,
        mptcp,
        smux,
        proxy_type,
        ..
    } = proxy;

    ProxyNodeView {
        record_id,
        name,
        proxy_type,
        history,
        extra,
        id,
        hidden,
        icon,
        test_url,
        capabilities: ProxyCapabilities {
            udp,
            xudp,
            tfo,
            mptcp,
            smux,
        },
        source,
    }
}

fn build_ordered_groups(
    mut core_groups: BTreeMap<String, Proxy>,
    runtime_group_order: Vec<String>,
    resolver: &MemberResolver<'_>,
) -> (Vec<ProxyGroupView>, ProxyViewOrderSource) {
    let mut groups = Vec::with_capacity(core_groups.len());
    for name in runtime_group_order {
        if let Some(proxy) = core_groups.remove(&name) {
            groups.push(build_group(name, proxy, resolver));
        }
    }

    let order_source = if groups.is_empty() {
        ProxyViewOrderSource::Fallback
    } else {
        ProxyViewOrderSource::Runtime
    };
    groups.extend(
        core_groups
            .into_iter()
            .map(|(name, proxy)| build_group(name, proxy, resolver)),
    );
    (groups, order_source)
}

fn build_standalone(core_node_ids: &BTreeMap<String, String>) -> Vec<String> {
    let mut standalone = ["DIRECT", "REJECT"]
        .into_iter()
        .filter_map(|name| core_node_ids.get(name).cloned())
        .collect::<Vec<_>>();
    standalone.extend(
        core_node_ids
            .iter()
            .filter(|(name, _)| name.as_str() != "DIRECT" && name.as_str() != "REJECT")
            .map(|(_, record_id)| record_id.clone()),
    );
    standalone
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use serde_json::json;

    fn enhancement_plan() -> anyhow::Result<EnhancementPlan> {
        let config = serde_yaml_ng::from_str(
            "proxies: [{name: fast, type: ss}, {name: slow, type: ss}]\nproxy-groups: [{name: LB, type: load-balance, strategy: round-robin, proxies: [slow, fast]}]",
        )?;
        let (_, plan) = crate::enhance::load_balance::transform(config, true)?;
        plan.ok_or_else(|| anyhow::anyhow!("expected enhancement plan"))
    }

    #[test]
    fn enhancement_keeps_original_members_provider_sources_and_user_names() -> anyhow::Result<()> {
        let plan = enhancement_plan()?;
        let proxies: Proxies = serde_json::from_value(json!({"proxies": {
            "GLOBAL": {"all": ["LB", "__verge_lb_user"], "now": "LB", "type": "Selector"},
            "LB": {"all": ["slow", "fast"], "now": "slow", "type": "LoadBalance"},
            "__verge_lb_user": {"name": "__verge_lb_user"},
            "fast": {"name": "fast", "provider-name": "provider"},
            "slow": {"name": "slow", "provider-name": "provider"}
        }}))?;
        let providers = serde_json::from_value(json!({"providers": {
            "other": {"vehicleType": "Inline", "proxies": [{"name": "fast"}]},
            "provider": {"vehicleType": "Inline", "proxies": [{"name": "fast"}, {"name": "slow"}]}
        }}))?;
        let view = ProxyViewBuilder::build_enhanced(
            ProxyViewInput {
                runtime_group_order: vec!["LB".into()],
                group_scopes: BTreeMap::from([(
                    "LB".into(),
                    GroupScope {
                        providers: vec!["provider".into()],
                        exclude_types: vec![],
                    },
                )]),
                proxies,
                providers: Some(providers),
            },
            Some(&plan),
            BTreeMap::new(),
        );
        assert_eq!(view.groups.len(), 1);
        assert_eq!(
            view.groups[0].members.iter().map(member_name).collect::<Vec<_>>(),
            ["slow", "fast"]
        );
        assert_eq!(view.groups[0].now.as_deref(), Some("slow"));
        let status = view.groups[0].enhancement.as_ref().context("missing native status")?;
        assert_eq!(status.state, "unsupported");
        assert!(status.reason.is_some());
        assert!(view.records.values().any(|node| node.name == "__verge_lb_user"));
        let global = view.global.context("global view")?;
        assert_eq!(global.now.as_deref(), Some("LB"));
        assert_eq!(
            global.members.iter().map(member_name).collect::<Vec<_>>(),
            ["LB", "__verge_lb_user"]
        );
        for member in &view.groups[0].members {
            let ProxyMemberRef::Node { record_id, .. } = member else {
                anyhow::bail!("expected original provider node")
            };
            assert!(
                matches!(&view.records[record_id].source, ProxyNodeSource::Provider { provider_name, .. } if provider_name == "provider")
            );
        }
        Ok(())
    }

    #[test]
    fn enhancement_ranks_finite_scores_with_deterministic_ties() -> anyhow::Result<()> {
        let plan = enhancement_plan()?;
        let proxies = serde_json::from_value(json!({"proxies": {
            "LB": {"all": ["unknown", "slow", "fast"], "type": "LoadBalance"},
            "fast": {"name": "fast"}, "slow": {"name": "slow"}, "unknown": {"name": "unknown"}
        }}))?;
        let candidate = |name: &str, score: f64| crate::core::load_balance::CandidateStatus {
            name: name.into(),
            score: Some(score),
            selected: true,
            reachable: true,
            fresh_health: true,
            proven: true,
            confidence: 1.0,
            mean_rate: 1.0,
            peak_rate: 1.0,
            latency_ms: Some(20.0),
            state: "selected".into(),
            cooldown_remaining_seconds: None,
            last_checked_seconds_ago: None,
        };
        let status = BTreeMap::from([(
            "LB".into(),
            GroupStatus {
                state: "active".into(),
                reason: None,
                candidates: vec![
                    candidate("slow", 80.0),
                    candidate("fast", 80.0),
                    candidate("unknown", f64::NAN),
                ],
            },
        )]);
        let view = ProxyViewBuilder::build_enhanced(
            ProxyViewInput {
                runtime_group_order: vec!["LB".into()],
                group_scopes: BTreeMap::new(),
                proxies,
                providers: None,
            },
            Some(&plan),
            status,
        );
        assert_eq!(
            view.groups[0].members.iter().map(member_name).collect::<Vec<_>>(),
            ["fast", "slow", "unknown"]
        );
        assert_eq!(
            view.groups[0].enhancement.as_ref().map(|status| status.state.as_str()),
            Some("active")
        );
        Ok(())
    }

    #[test]
    fn unsupported_groups_keep_original_members_and_explain_why() -> anyhow::Result<()> {
        let config = serde_yaml_ng::from_str(
            "proxy-groups: [{name: LB, type: load-balance, strategy: round-robin, proxies: [DIRECT]}]",
        )?;
        let (_, plan) = crate::enhance::load_balance::transform(config, true)?;
        let proxies = serde_json::from_value(json!({"proxies": {
            "LB": {"all": ["DIRECT"], "type": "LoadBalance"}, "DIRECT": {"name": "DIRECT", "type": "Direct"}
        }}))?;
        let view = ProxyViewBuilder::build_enhanced(
            ProxyViewInput {
                runtime_group_order: vec!["LB".into()],
                group_scopes: BTreeMap::new(),
                proxies,
                providers: None,
            },
            plan.as_ref(),
            BTreeMap::new(),
        );
        assert_eq!(
            view.groups[0].members.iter().map(member_name).collect::<Vec<_>>(),
            ["DIRECT"]
        );
        let status = view.groups[0].enhancement.as_ref().context("unsupported status")?;
        assert_eq!(status.state, "unsupported");
        assert!(status.reason.as_ref().is_some_and(|reason| !reason.is_empty()));
        Ok(())
    }

    #[test]
    fn provider_echo_in_proxies_does_not_override_group_membership() -> anyhow::Result<()> {
        let view = ProxyViewBuilder::build(ProxyViewInput {
            runtime_group_order: vec!["A".into(), "B".into()],
            group_scopes: ["A", "B"]
                .into_iter()
                .map(|name| {
                    (
                        name.into(),
                        GroupScope {
                            providers: vec![name.into()],
                            exclude_types: vec![],
                        },
                    )
                })
                .collect(),
            proxies: serde_json::from_value(json!({"proxies": {
                "A": {"all": ["node"], "type": "Selector"},
                "B": {"all": ["node"], "type": "Selector"},
                "node": {"name": "node", "provider-name": "A", "history": [{"time": "2026-10-08T00:00:00Z", "delay": 32}]}
            }}))?,
            providers: Some(serde_json::from_value(json!({"providers": {
                "A": {"vehicleType": "Inline", "proxies": [{"name": "node", "history": [{"time": "2026-10-08T00:00:00Z", "delay": 32}]}]},
                "B": {"vehicleType": "Inline", "proxies": [{"name": "node", "history": [{"time": "2026-10-08T00:00:00Z", "delay": 123}]}]}
            }}))?),
        });
        for (group, expected) in view.groups.iter().zip([32, 123]) {
            let ProxyMemberRef::Node { record_id, .. } = &group.members[0] else {
                anyhow::bail!("Expected node")
            };
            let record = &view.records[record_id];
            assert_eq!(record.history.last().map(|entry| entry.delay), Some(expected));
            assert!(
                matches!(&record.source, ProxyNodeSource::Provider { provider_name, .. } if provider_name == &group.name)
            );
        }
        Ok(())
    }
    #[test]
    fn unavailable_provider_keeps_core_echo_interactable() -> anyhow::Result<()> {
        let view = ProxyViewBuilder::build(ProxyViewInput {
            runtime_group_order: vec!["A".into()],
            group_scopes: BTreeMap::new(),
            proxies: serde_json::from_value(json!({"proxies": {
                "A": {"all": ["node"], "type": "Selector"},
                "node": {"name": "node", "provider-name": "provider"}
            }}))?,
            providers: None,
        });
        assert!(matches!(&view.groups[0].members[0], ProxyMemberRef::Node { .. }));
        Ok(())
    }
}
