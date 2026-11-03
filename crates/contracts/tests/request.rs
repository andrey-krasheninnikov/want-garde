use garde_contracts::{Action, ErrorCode, decode_request};

#[test]
fn decodes_a_status_request_without_granting_authority() {
    let request = decode_request(include_str!("../../../fixtures/contracts/request.json")).unwrap();
    assert_eq!(request.request_id.as_str(), "request-001");
    assert_eq!(request.server_id.as_str(), "server-demo");
    assert_eq!(request.action, Action::ServerStatus {});
}

#[test]
fn rejects_client_authorization_claims_without_echoing_the_input() {
    let input = r#"{"version":"1","request_id":"request-001","server_id":"server-demo","action":{"type":"server.status"},"approved":true,"token":"sensitive-example"}"#;
    let error = decode_request(input).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("sensitive-example")
    );
}

#[test]
fn rejects_unknown_versions_actions_fields_and_malformed_identifiers() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/invalid-requests.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let result = decode_request(&case["request"].to_string());
        assert!(result.is_err(), "{}", case["case"]);
    }
}

#[test]
fn bounds_even_valid_json_whitespace_at_the_transport_boundary() {
    let request = include_str!("../../../fixtures/contracts/request.json");
    let at_limit = format!("{}{}", request, " ".repeat(65536 - request.len()));
    assert!(decode_request(&at_limit).is_ok());
    assert!(decode_request(&(at_limit + " ")).is_err());
}

#[test]
fn decodes_read_only_fail2ban_status_and_rejects_operation_parameters() {
    let input = r#"{"version":"1","request_id":"bans-001","server_id":"server-demo","action":{"type":"fail2ban.status"}}"#;
    assert_eq!(
        decode_request(input).unwrap().action,
        Action::Fail2banStatus {}
    );
    assert!(
        decode_request(&input.replace(
            "\"fail2ban.status\"",
            "\"fail2ban.status\",\"command\":\"unbanip\""
        ))
        .is_err()
    );
}
