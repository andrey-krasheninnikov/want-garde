use garde_contracts::{Event, Grant, Incident, Job};
use serde::{Serialize, de::DeserializeOwned};

fn roundtrip<T: DeserializeOwned + Serialize>(input: &str) {
    let expected: serde_json::Value = serde_json::from_str(input).unwrap();
    let decoded: T = serde_json::from_str(input).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn shared_observations_permissions_and_results_preserve_the_wire_examples() {
    roundtrip::<Event>(include_str!("../../../fixtures/contracts/event.json"));
    roundtrip::<Incident>(include_str!("../../../fixtures/contracts/incident.json"));
    roundtrip::<Grant>(include_str!("../../../fixtures/contracts/grant.json"));
    roundtrip::<Job>(include_str!("../../../fixtures/contracts/job.json"));
}

#[test]
fn a_closed_incident_requires_a_reason_and_an_unknown_job_state_is_rejected() {
    let mut incident: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/contracts/incident.json")).unwrap();
    incident["state"].as_object_mut().unwrap().remove("reason");
    assert!(serde_json::from_value::<Incident>(incident).is_err());
    let mut job: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/contracts/job.json")).unwrap();
    job["state"]["type"] = serde_json::json!("pretend_success");
    assert!(serde_json::from_value::<Job>(job).is_err());
}
