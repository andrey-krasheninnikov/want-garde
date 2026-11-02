use garde_contracts::Measurement;
use garde_daemon::{StatusSource, host::LinuxHost};
use std::fs;

#[tokio::test]
async fn source_failures_are_explicit_and_do_not_replace_valid_host_measurements() {
    let dir = tempfile::tempdir().unwrap();
    let proc = dir.path().join("proc");
    fs::create_dir_all(proc.join("sys/kernel")).unwrap();
    for (name, value) in [
        ("sys/kernel/hostname", "тест-host\n"),
        ("sys/kernel/osrelease", "6.0-test\n"),
        ("uptime", "12.9 88.0\n"),
        ("loadavg", "1.00 2.50 0.25 1/2 3\n"),
        ("meminfo", "MemTotal: 1024 kB\nMemAvailable: 512 kB\n"),
        ("stat", "cpu 100 0 100 800 0 0 0 0 0 0\n"),
    ] {
        fs::write(proc.join(name), value).unwrap();
    }
    let os = dir.path().join("os-release");
    fs::write(&os, "PRETTY_NAME=\"Test Linux\"\n").unwrap();
    let host = LinuxHost::from_paths(proc.clone(), os, dir.path().join("missing-disk"));
    let snapshot = host.snapshot().await;
    assert_eq!(
        snapshot.hostname,
        Measurement::Available {
            value: "тест-host".into()
        }
    );
    assert_eq!(
        snapshot.uptime_seconds,
        Measurement::Available { value: 12 }
    );
    assert_eq!(
        snapshot.load_average,
        Measurement::Available {
            value: [1.0, 2.5, 0.25]
        }
    );
    let Measurement::Available { value } = snapshot.memory else {
        panic!("memory_unavailable")
    };
    assert_eq!(
        (value.total_bytes, value.available_bytes),
        (1048576, 524288)
    );
    assert!(matches!(
        snapshot.root_disk,
        Measurement::Unavailable { .. }
    ));
    assert!(matches!(
        snapshot.cpu_usage_percent,
        Measurement::Unavailable { .. }
    ));
    fs::write(proc.join("meminfo"), "MemTotal: 1 kB\nMemAvailable: 2 kB\n").unwrap();
    fs::write(proc.join("loadavg"), "NaN 1 2\n").unwrap();
    let snapshot = host.snapshot().await;
    assert!(matches!(snapshot.memory, Measurement::Unavailable { .. }));
    assert!(matches!(
        snapshot.load_average,
        Measurement::Unavailable { .. }
    ));
    assert_eq!(
        snapshot.uptime_seconds,
        Measurement::Available { value: 12 }
    );
}
