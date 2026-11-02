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

#[test]
fn payload_free_job_states_reject_extra_fields_but_accept_the_documented_shape() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/invalid-states.json"
    ))
    .unwrap();
    for case in cases
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["schema"] == "job")
    {
        assert!(
            serde_json::from_value::<Job>(case["value"].clone()).is_err(),
            "{}",
            case["case"]
        );
        let mut valid = case["value"].clone();
        valid["state"].as_object_mut().unwrap().remove("unexpected");
        let decoded: Job = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), valid);
    }
}

#[test]
fn open_incident_rejects_extra_fields_but_accepts_the_documented_shape() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/invalid-states.json"
    ))
    .unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["schema"] == "incident")
        .unwrap();
    assert!(serde_json::from_value::<Incident>(case["value"].clone()).is_err());
    let mut valid = case["value"].clone();
    valid["state"].as_object_mut().unwrap().remove("unexpected");
    let decoded: Incident = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), valid);
}
