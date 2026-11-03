#![cfg(target_os = "linux")]
use garde_daemon::firewall::{BansSource, HelperClient};
fn isolated() {
    assert_eq!(
        std::env::var("WANT_GARDE_TEST_HELPER_TASK").as_deref(),
        Ok("GH-6")
    );
}
#[tokio::test]
#[ignore = "requires the task-owned root helper socket and an allowed service UID"]
async fn native_root_helper_status_is_typed_and_read_only() {
    isolated();
    let snapshot = HelperClient.snapshot().await.expect("native_helper_status");
    assert!(snapshot.jails.len() <= 32);
    assert!(snapshot.log_target.is_some());
    assert!(snapshot.log_level.is_some());
    assert!(
        snapshot
            .jails
            .iter()
            .flat_map(|j| &j.addresses)
            .all(|ip| ip.parse::<std::net::IpAddr>().is_ok())
    );
}
#[tokio::test]
#[ignore = "requires the task-owned root helper socket and a denied UID or policy"]
async fn native_root_helper_denies_an_unapproved_identity() {
    isolated();
    assert!(
        HelperClient.snapshot().await.is_err(),
        "unexpected_helper_access"
    );
}
