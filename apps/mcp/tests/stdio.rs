use std::process::Command;

#[test]
fn unavailable_mcp_does_not_contaminate_protocol_stdout() {
    let result = Command::new(env!("CARGO_BIN_EXE_want-garde-mcp"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
}
