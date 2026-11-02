use std::{fs, process::Command};

fn tool(output: &std::path::Path, check: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command.args(["contracts", "--output-dir"]).arg(output);
    if check {
        command.arg("--check");
    }
    command.output().unwrap()
}

#[test]
fn exports_consumable_schemas_and_check_detects_drift_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("contracts");
    assert!(tool(&output, false).status.success());
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(output.join("operation-request.schema.json")).unwrap(),
    )
    .unwrap();
    let request: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/contracts/request.json")).unwrap();
    assert!(jsonschema::is_valid(&schema, &request));
    assert!(tool(&output, true).status.success());
    fs::write(output.join("index.ts"), "stale-contract").unwrap();
    assert!(!tool(&output, true).status.success());
    assert_eq!(
        fs::read_to_string(output.join("index.ts")).unwrap(),
        "stale-contract"
    );
}

#[test]
fn check_does_not_create_missing_artifacts_and_invalid_arguments_fail() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("absent");
    assert!(!tool(&output, true).status.success());
    assert!(!output.exists());
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("unknown")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
}

#[test]
fn schemas_match_valid_examples_and_reject_the_negative_request_corpus() {
    let directory = tempfile::tempdir().unwrap();
    assert!(tool(directory.path(), false).status.success());
    for (schema_name, input) in [
        (
            "operation-request",
            include_str!("../../../fixtures/contracts/request.json"),
        ),
        (
            "event",
            include_str!("../../../fixtures/contracts/event.json"),
        ),
        (
            "incident",
            include_str!("../../../fixtures/contracts/incident.json"),
        ),
        (
            "grant",
            include_str!("../../../fixtures/contracts/grant.json"),
        ),
        ("job", include_str!("../../../fixtures/contracts/job.json")),
        (
            "host-snapshot",
            include_str!("../../../fixtures/contracts/host-snapshot.json"),
        ),
    ] {
        let schema: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(directory.path().join(format!("{schema_name}.schema.json")))
                .unwrap(),
        )
        .unwrap();
        let instance: serde_json::Value = serde_json::from_str(input).unwrap();
        assert!(jsonschema::is_valid(&schema, &instance), "{schema_name}");
    }
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("operation-request.schema.json")).unwrap(),
    )
    .unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/invalid-requests.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        assert!(
            !jsonschema::is_valid(&schema, &case["request"]),
            "{}",
            case["case"]
        );
    }
}

#[test]
fn schemas_reject_extra_state_fields_and_accept_payload_free_states() {
    let directory = tempfile::tempdir().unwrap();
    assert!(tool(directory.path(), false).status.success());
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/invalid-states.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let name = case["schema"].as_str().unwrap();
        let schema: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(directory.path().join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert!(
            !jsonschema::is_valid(&schema, &case["value"]),
            "{}",
            case["case"]
        );
        let mut valid = case["value"].clone();
        valid["state"].as_object_mut().unwrap().remove("unexpected");
        assert!(jsonschema::is_valid(&schema, &valid), "{}", case["case"]);
    }
}
