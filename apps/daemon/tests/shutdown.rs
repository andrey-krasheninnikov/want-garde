use std::{
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant},
};

#[test]
fn process_exit_is_bounded_when_native_blocking_work_is_still_running() {
    if std::env::var_os("GARDE_SHUTDOWN_CHILD").is_some() {
        assert_eq!(
            garde_daemon::run_process(async {
                let (tx, rx) = std::sync::mpsc::channel();
                tokio::task::spawn_blocking(move || {
                    tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_secs(10));
                });
                rx.recv().unwrap();
                ExitCode::SUCCESS
            }),
            ExitCode::SUCCESS
        );
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process_exit_is_bounded_when_native_blocking_work_is_still_running",
        ])
        .env("GARDE_SHUTDOWN_CHILD", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("process_exit_waited_for_blocking_work");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
