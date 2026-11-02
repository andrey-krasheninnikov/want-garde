use garde_contracts::{HostSnapshot, Measurement};

#[test]
fn snapshot_keeps_measurement_failures_separate_from_zero_values() {
    let input = include_str!("../../../fixtures/contracts/host-snapshot.json");
    let snapshot: HostSnapshot = serde_json::from_str(input).unwrap();
    assert!(matches!(
        snapshot.uptime_seconds,
        Measurement::Available { value: 0 }
    ));
    assert!(matches!(
        snapshot.root_disk,
        Measurement::Unavailable { .. }
    ));
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        serde_json::from_str::<serde_json::Value>(input).unwrap()
    );
}
