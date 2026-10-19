use super::{CmdResult, WithErrorCode as _};
use crate::{config::cloudflare_speed_test::*, core::cloudflare_speed_test, feat::cloudflare_dns};

#[tauri::command]
#[specta::specta]
pub async fn get_cloudflare_speed_test() -> CmdResult<CloudflareSpeedTestView> {
    cloudflare_speed_test::get()
        .await
        .with_error_code("CLOUDFLARE_TEST_UNAVAILABLE")
}

#[tauri::command]
#[specta::specta]
pub async fn start_cloudflare_speed_test(url: String, interface_name: Option<String>) -> CmdResult<CloudflareSnapshot> {
    cloudflare_speed_test::start(url, interface_name)
        .await
        .with_error_code("CLOUDFLARE_TEST_START_FAILED")
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_cloudflare_speed_test(run_id: String) -> CmdResult<CloudflareSnapshot> {
    cloudflare_speed_test::cancel(&run_id)
        .await
        .with_error_code("CLOUDFLARE_TEST_CANCEL_FAILED")
}

#[tauri::command]
#[specta::specta]
pub async fn get_cloudflare_dns_inventory() -> CmdResult<CloudflareDnsInventory> {
    cloudflare_dns::get_cloudflare_dns_inventory()
        .await
        .with_error_code("CLOUDFLARE_INVENTORY_UNAVAILABLE")
}

#[tauri::command]
#[specta::specta]
pub async fn set_cloudflare_dns_bindings(
    profile_uid: String,
    fingerprint: String,
    bindings: Vec<CloudflareDnsBinding>,
    enabled: bool,
    confirmation: Option<String>,
) -> CmdResult<CloudflareDnsOutcome> {
    cloudflare_dns::set_cloudflare_dns_bindings(profile_uid, fingerprint, bindings, enabled, confirmation)
        .await
        .with_error_code("CLOUDFLARE_BINDING_FAILED")
}
