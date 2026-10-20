use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "verge-cloudflare1";
pub const LATENCY_METHOD: &str = "tcp-connect";
pub const METHOD: &str = "verge-cfst-transfer-v3";
pub const BUDGET_BYTES: u64 = 256 * 1024 * 1024;
pub const PREFERRED_DOMAINS: [&str; 3] = [
    "best.cloudflare.182682.xyz",
    "saas.sin.fan",
    "best.cloudflare.19931110.xyz",
];

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareCapability {
    pub schema_version: u32,
    pub capability: String,
    pub supported: bool,
    pub preferred_domains: Vec<String>,
    pub budget_bytes: u64,
    #[serde(default)]
    pub domain_only: bool,
    #[serde(default)]
    pub engine_version: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub latency_method: String,
    #[serde(default)]
    pub dns_aliases: bool,
    #[serde(default)]
    pub fixed_address_family: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl CloudflareCapability {
    pub fn unsupported(reason: &str) -> Self {
        Self {
            schema_version: 1,
            capability: CAPABILITY.into(),
            supported: false,
            preferred_domains: PREFERRED_DOMAINS.iter().map(|domain| (*domain).into()).collect(),
            budget_bytes: BUDGET_BYTES,
            domain_only: false,
            engine_version: String::new(),
            method: String::new(),
            latency_method: String::new(),
            dns_aliases: false,
            fixed_address_family: false,
            reason: Some(reason.into()),
        }
    }

    pub fn compatible(&self) -> bool {
        self.supported && self.schema_version == 1 && self.capability == CAPABILITY
    }

    pub fn supports_domain_only_test(&self) -> bool {
        self.compatible()
            && self.domain_only
            && self.engine_version == "cf-speedtest3"
            && self.method == METHOD
            && self.latency_method == LATENCY_METHOD
            && self.preferred_domains.iter().map(String::as_str).eq(PREFERRED_DOMAINS)
            && self.budget_bytes > 0
            && self.budget_bytes <= BUDGET_BYTES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_capability_preserves_dns_support_but_cannot_start_domain_only_testing() {
        let mut capability: CloudflareCapability = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "capability": CAPABILITY, "supported": true,
            "preferredDomains": PREFERRED_DOMAINS, "budgetBytes": BUDGET_BYTES,
            "dnsAliases": true, "fixedAddressFamily": true
        }))
        .unwrap();
        assert!(capability.compatible() && capability.dns_aliases && capability.fixed_address_family);
        assert!(!capability.supports_domain_only_test());
        capability.domain_only = true;
        capability.engine_version = "cf-speedtest2".into();
        capability.method = "verge-cfst-transfer-v2".into();
        assert!(!capability.supports_domain_only_test());
        assert!(capability.compatible());
        capability.latency_method = LATENCY_METHOD.into();
        assert!(!capability.supports_domain_only_test());
        assert!(capability.compatible() && capability.dns_aliases && capability.fixed_address_family);
        capability.engine_version = "cf-speedtest3".into();
        assert!(!capability.supports_domain_only_test());
        capability.method = METHOD.into();
        assert!(capability.supports_domain_only_test());
        capability.preferred_domains.push("untrusted.example".into());
        assert!(!capability.supports_domain_only_test());
        capability.preferred_domains.pop();
        capability.budget_bytes = BUDGET_BYTES + 1;
        assert!(!capability.supports_domain_only_test());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareRunState {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareStage {
    Resolving,
    Screening,
    Downloading,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareResultState {
    Pending,
    Screened,
    Complete,
    Short,
    Failed,
    Untested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareDiagnosticStage {
    Dns,
    Interface,
    Tcp,
    Tls,
    Headers,
    Http,
    Content,
    Body,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareDiagnosticCode {
    ResolutionFailed,
    NoEligibleAddresses,
    InterfaceChanged,
    TcpFailed,
    TlsFailed,
    SetupTimeout,
    RequestFailed,
    HttpStatus,
    RateLimited,
    UnsupportedEncoding,
    UnsupportedContent,
    BodyReadFailed,
    NoMeasuredData,
    Cancelled,
    DeadlineExceeded,
    NoCandidates,
    NoCompletedResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareContentType {
    Binary,
    Plain,
    Html,
    Json,
    Other,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareDiagnostic {
    pub stage: CloudflareDiagnosticStage,
    pub code: CloudflareDiagnosticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<CloudflareContentType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareFinishReason {
    Eof,
    Quota,
    Window,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareResult {
    pub ip: String,
    pub sources: Vec<String>,
    pub state: CloudflareResultState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub bytes_read: u64,
    pub duration_ms: u64,
    pub failures: u32,
    pub attempts: u32,
    pub successful_rounds: u32,
    pub attempted_rounds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<CloudflareFinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<CloudflareDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareSource {
    pub name: String,
    pub ips: Vec<String>,
    pub state: String,
    pub eligible_ipv4: u32,
    pub eligible_ipv6: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<CloudflareDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareSnapshot {
    pub schema_version: u32,
    pub capability: String,
    pub method: String,
    #[serde(default)]
    pub latency_method: String,
    pub run_id: String,
    pub revision: u64,
    pub state: CloudflareRunState,
    pub stage: CloudflareStage,
    pub interface_name: String,
    pub dns_mode: String,
    pub url: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub bytes_read: u64,
    pub budget_bytes: u64,
    pub completed_targets: u32,
    pub total_targets: u32,
    pub results: Vec<CloudflareResult>,
    pub sources: Vec<CloudflareSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<CloudflareDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareSpeedTestUnavailable {
    pub run_id: String,
    pub revision: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CloudflareSpeedTestView {
    pub capability: CloudflareCapability,
    pub snapshot: Option<CloudflareSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloudflareBindingTarget {
    PreferredDomain {
        domain: String,
    },
    FixedIp {
        ip: String,
        #[serde(rename = "runId")]
        run_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CloudflareDnsBinding {
    pub domain: String,
    pub target: CloudflareBindingTarget,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct ProfileCloudflareDnsSettings {
    pub enabled: bool,
    pub bindings: Vec<CloudflareDnsBinding>,
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[expect(
    dead_code,
    reason = "Legacy occurrence roles remain part of the shared wire contract"
)]
pub enum CloudflareOccurrenceRole {
    Url,
    Dns,
    ProxyServer,
    Identity,
    Subscription,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareOccurrence {
    pub id: String,
    pub display: String,
    pub field_path: String,
    pub role: CloudflareOccurrenceRole,
    pub bindable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CloudflareDomain {
    pub domain: String,
    pub occurrences: Vec<CloudflareOccurrence>,
    pub bindable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CloudflareDnsRestoreStatus {
    Pending,
    Restoring,
    Active,
    Dormant,
    Error,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareDnsInventory {
    pub profile_uid: String,
    pub fingerprint: String,
    pub domains: Vec<CloudflareDomain>,
    pub bindings: Vec<CloudflareDnsBinding>,
    pub enabled: bool,
    pub effective_enabled: bool,
    pub applied_domains: Vec<String>,
    pub dormant_domains: Vec<String>,
    pub restore_status: CloudflareDnsRestoreStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_reason: Option<String>,
    pub use_hosts: Option<bool>,
    pub unknown_sources: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CloudflareDnsOutcome {
    Applied {
        saved: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Stale {
        reason: String,
    },
    ConfirmationRequired {
        confirmation: String,
        conflicts: Vec<String>,
        enable_use_hosts: bool,
        affected_domains: Vec<String>,
        warnings: Vec<String>,
    },
}
