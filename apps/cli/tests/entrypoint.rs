use std::process::Command;

#[test]
fn shows_version_localized_help_and_fails_closed_without_echoing_input() {
    let binary = env!("CARGO_BIN_EXE_want-garde");
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("want-garde {}\n", env!("CARGO_PKG_VERSION"))
    );
    let help = Command::new(binary)
        .args(["--lang", "ru", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("owner bind")
    );
    let result = Command::new(binary)
        .arg("sensitive-example")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(
        !String::from_utf8(result.stderr)
            .unwrap()
            .contains("sensitive-example")
    );
}
