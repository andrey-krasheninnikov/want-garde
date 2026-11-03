use crate::StatusSource;
use garde_contracts::{
    ByteUsage, DiskMeasurement, DiskUsage, HostSnapshot, Id, InodeUsage, Measurement,
};
use std::sync::Arc;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex,
};

pub struct HostSources {
    pub proc_root: PathBuf,
    pub os_release: PathBuf,
    pub reboot_marker: PathBuf,
    pub reboot_producer: PathBuf,
    pub filesystem_program: PathBuf,
}
impl Default for HostSources {
    fn default() -> Self {
        Self {
            proc_root: "/proc".into(),
            os_release: "/etc/os-release".into(),
            reboot_marker: "/run/reboot-required".into(),
            reboot_producer: "/usr/share/update-notifier/notify-reboot-required".into(),
            filesystem_program: "/usr/bin/stat".into(),
        }
    }
}

pub struct LinuxHost {
    sources: HostSources,
    sample_lock: Mutex<()>,
    disks: Vec<(PathBuf, Arc<Mutex<Option<Child>>>)>,
}
impl Default for LinuxHost {
    fn default() -> Self {
        Self::with_sources(HostSources::default(), vec!["/".into()])
    }
}
impl LinuxHost {
    pub fn from_paths(proc_root: PathBuf, os_release: PathBuf, disk: PathBuf) -> Self {
        Self::with_sources(
            HostSources {
                proc_root,
                os_release,
                ..Default::default()
            },
            vec![disk],
        )
    }
    pub fn with_sources(sources: HostSources, disks: Vec<PathBuf>) -> Self {
        Self {
            sources,
            sample_lock: Mutex::new(()),
            disks: disks
                .into_iter()
                .take(8)
                .map(|path| (path, Arc::new(Mutex::new(None))))
                .collect(),
        }
    }
}

fn unavailable<T>(code: &str) -> Measurement<T> {
    Measurement::Unavailable {
        reason_key: Id::new(code).expect("static_identifier"),
    }
}

async fn filesystem(
    path: PathBuf,
    program: PathBuf,
    slot: Arc<Mutex<Option<Child>>>,
) -> DiskMeasurement {
    let failed = |code| DiskMeasurement {
        path: path.to_string_lossy().into_owned(),
        space: unavailable(code),
        inodes: unavailable(code),
    };
    let Ok(mut slot) = slot.try_lock() else {
        return failed("host.probe_busy");
    };
    if let Some(child) = slot.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                let _ = child.start_kill();
                return failed("host.probe_busy");
            }
        }
    }
    *slot = match Command::new(program)
        .env_clear()
        .env("LC_ALL", "C")
        .args(["--file-system", "--format=%S %b %f %a %c %d", "--"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => Some(child),
        Err(_) => return failed("host.probe_unavailable"),
    };
    let child = slot.as_mut().expect("spawned_child");
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        child
            .stdout
            .as_mut()
            .ok_or(())?
            .take(4097)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ())?;
        if bytes.len() > 4096 {
            return Err(());
        }
        let status = child.wait().await.map_err(|_| ())?;
        if status.success() { Ok(()) } else { Err(()) }
    })
    .await;
    let code = match result {
        Ok(Ok(())) => None,
        Ok(Err(())) => Some("host.filesystem_unavailable"),
        Err(_) => Some("host.probe_timeout"),
    };
    if let Some(code) = code {
        let _ = child.start_kill();
        return failed(code);
    }
    *slot = None;
    let numbers = std::str::from_utf8(&bytes).ok().and_then(|s| {
        s.split_whitespace()
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
    });
    let Some(n) = numbers.filter(|n| n.len() == 6) else {
        return failed("host.probe_invalid");
    };
    let space = n[1]
        .checked_mul(n[0])
        .zip(n[2].checked_mul(n[0]))
        .zip(n[3].checked_mul(n[0]))
        .filter(|((total, free), available)| {
            *total > 0 && free <= total && available <= free && *total < (1 << 53)
        })
        .map(|((total, free), available)| DiskUsage {
            total_bytes: total,
            used_bytes: total - free,
            available_bytes: available,
        });
    let inodes = (n[4] > 0 && n[4] < (1 << 53) && n[5] <= n[4]).then_some(InodeUsage {
        total: n[4],
        available: n[5],
    });
    DiskMeasurement {
        path: path.to_string_lossy().into_owned(),
        space: measurement(space),
        inodes: measurement(inodes),
    }
}

fn text(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(65537)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 65536 {
        return None;
    }
    String::from_utf8(bytes).ok()
}
fn measurement<T>(value: Option<T>) -> Measurement<T> {
    match value {
        Some(value) => Measurement::Available { value },
        None => Measurement::Unavailable {
            reason_key: Id::new("source.unavailable").expect("static_identifier"),
        },
    }
}
fn cpu(path: &Path) -> Option<(u64, u64)> {
    let input = text(path)?;
    let mut fields = input.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let numbers: Vec<_> = fields
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    if numbers.len() < 4 {
        return None;
    }
    let total = numbers
        .iter()
        .try_fold(0u64, |sum, n| sum.checked_add(*n))?;
    let idle = numbers[3].checked_add(*numbers.get(4).unwrap_or(&0))?;
    Some((total, idle))
}
fn finite(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}
fn usage(total: u64, available: u64) -> Option<ByteUsage> {
    (total > 0 && available <= total && total < (1 << 53)).then_some(ByteUsage {
        total_bytes: total,
        available_bytes: available,
    })
}

impl StatusSource for LinuxHost {
    async fn snapshot(&self) -> HostSnapshot {
        let _sample = self.sample_lock.lock().await;
        let cpu_path = self.sources.proc_root.join("stat");
        let start = cpu(&cpu_path);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cpu_percent = start.zip(cpu(&cpu_path)).and_then(|((a, b), (c, d))| {
            let total = c.checked_sub(a)?;
            let idle = d.checked_sub(b)?;
            if total == 0 || idle > total {
                return None;
            }
            Some(100.0 * (total - idle) as f64 / total as f64)
        });
        let load = text(&self.sources.proc_root.join("loadavg")).and_then(|v| {
            let values: Vec<_> = v
                .split_whitespace()
                .take(3)
                .map(finite)
                .collect::<Option<_>>()?;
            values.try_into().ok()
        });
        let memory = text(&self.sources.proc_root.join("meminfo")).and_then(|v| {
            let bytes = |key: &str| -> Option<u64> {
                let mut fields = v
                    .lines()
                    .find(|line| line.starts_with(key))?
                    .split_whitespace();
                fields.next()?;
                let n = fields.next()?.parse::<u64>().ok()?;
                if fields.next()? != "kB" {
                    return None;
                }
                n.checked_mul(1024)
            };
            usage(bytes("MemTotal:")?, bytes("MemAvailable:")?)
        });
        let mut probes = tokio::task::JoinSet::new();
        for (index, (path, slot)) in self.disks.iter().enumerate() {
            let (path, program, slot) = (
                path.clone(),
                self.sources.filesystem_program.clone(),
                slot.clone(),
            );
            probes.spawn(async move { (index, filesystem(path, program, slot).await) });
        }
        let mut disks = Vec::new();
        while let Some(result) = probes.join_next().await {
            if let Ok(result) = result {
                disks.push(result);
            }
        }
        disks.sort_by_key(|(index, _)| *index);
        let disks: Vec<_> = disks.into_iter().map(|(_, disk)| disk).collect();
        let disk = disks
            .iter()
            .find(|disk| disk.path == "/")
            .and_then(|disk| match &disk.space {
                Measurement::Available { value } => Some(ByteUsage {
                    total_bytes: value.total_bytes,
                    available_bytes: value.available_bytes,
                }),
                _ => None,
            });
        let cores = text(&cpu_path).and_then(|s| {
            let count = s
                .lines()
                .filter(|line| {
                    line.split_whitespace().next().is_some_and(|label| {
                        label.strip_prefix("cpu").is_some_and(|suffix| {
                            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                        })
                    })
                })
                .count();
            u32::try_from(count).ok().filter(|n| *n > 0)
        });
        let load_per_core = load
            .zip(cores)
            .map(|(load, cores): ([f64; 3], u32)| load.map(|v| v / cores as f64));
        let os_id = text(&self.sources.os_release).and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("ID=")
                    .map(|id| id.trim_matches('"').to_owned())
            })
        });
        let reboot_required = if matches!(os_id.as_deref(), Some("debian" | "ubuntu")) {
            match std::fs::symlink_metadata(&self.sources.reboot_marker) {
                Ok(metadata) if metadata.is_file() => Measurement::Available { value: true },
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && self.sources.reboot_producer.is_file() =>
                {
                    Measurement::Available { value: false }
                }
                _ => unavailable("host.reboot_source_unavailable"),
            }
        } else {
            unavailable("host.reboot_unsupported")
        };
        let os = text(&self.sources.os_release)
            .and_then(|v| {
                v.lines().find_map(|l| {
                    l.strip_prefix("PRETTY_NAME=")
                        .map(|v| v.trim_matches('"').to_owned())
                })
            })
            .filter(|v| !v.is_empty());
        let uptime = text(&self.sources.proc_root.join("uptime"))
            .and_then(|v| finite(v.split_whitespace().next()?))
            .filter(|v| *v <= u32::MAX as f64)
            .map(|v| v as u32);
        HostSnapshot {
            observed_at: SystemTime::now().into(),
            hostname: measurement(
                text(&self.sources.proc_root.join("sys/kernel/hostname"))
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty()),
            ),
            os: measurement(os),
            kernel: measurement(
                text(&self.sources.proc_root.join("sys/kernel/osrelease"))
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty()),
            ),
            uptime_seconds: measurement(uptime),
            cpu_usage_percent: measurement(cpu_percent),
            load_average: measurement(load),
            memory: measurement(memory),
            root_disk: measurement(disk),
            cpu_count: measurement(cores),
            load_per_core: measurement(load_per_core),
            disks,
            reboot_required,
        }
    }
}
