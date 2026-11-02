use crate::StatusSource;
use garde_contracts::{ByteUsage, HostSnapshot, Id, Measurement};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub struct LinuxHost {
    proc_root: PathBuf,
    os_release: PathBuf,
    disk: PathBuf,
}
impl Default for LinuxHost {
    fn default() -> Self {
        Self::from_paths("/proc".into(), "/etc/os-release".into(), "/".into())
    }
}
impl LinuxHost {
    pub fn from_paths(proc_root: PathBuf, os_release: PathBuf, disk: PathBuf) -> Self {
        Self {
            proc_root,
            os_release,
            disk,
        }
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
        let cpu_path = self.proc_root.join("stat");
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
        let load = text(&self.proc_root.join("loadavg")).and_then(|v| {
            let values: Vec<_> = v
                .split_whitespace()
                .take(3)
                .map(finite)
                .collect::<Option<_>>()?;
            values.try_into().ok()
        });
        let memory = text(&self.proc_root.join("meminfo")).and_then(|v| {
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
        let disk = rustix::fs::statvfs(&self.disk).ok().and_then(|s| {
            usage(
                s.f_blocks.checked_mul(s.f_frsize)?,
                s.f_bavail.checked_mul(s.f_frsize)?,
            )
        });
        let os = text(&self.os_release)
            .and_then(|v| {
                v.lines().find_map(|l| {
                    l.strip_prefix("PRETTY_NAME=")
                        .map(|v| v.trim_matches('"').to_owned())
                })
            })
            .filter(|v| !v.is_empty());
        let uptime = text(&self.proc_root.join("uptime"))
            .and_then(|v| finite(v.split_whitespace().next()?))
            .filter(|v| *v <= u32::MAX as f64)
            .map(|v| v as u32);
        HostSnapshot {
            observed_at: SystemTime::now().into(),
            hostname: measurement(
                text(&self.proc_root.join("sys/kernel/hostname"))
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty()),
            ),
            os: measurement(os),
            kernel: measurement(
                text(&self.proc_root.join("sys/kernel/osrelease"))
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty()),
            ),
            uptime_seconds: measurement(uptime),
            cpu_usage_percent: measurement(cpu_percent),
            load_average: measurement(load),
            memory: measurement(memory),
            root_disk: measurement(disk),
        }
    }
}
