//! Bounded sampling of this daemon and its descendants, never unrelated host workloads.
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
pub(super) struct HostMetrics {
    pub available: bool,
    pub scope: &'static str,
    pub hostname: Option<String>,
    pub platform: String,
    pub cpu_cores: Option<usize>,
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_limit_is_cgroup: bool,
    pub disk_used_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub child_process_count: Option<usize>,
    pub child_rss_bytes: Option<u64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub sample_ms: u64,
}

#[cfg(target_os = "linux")]
fn bounded_read(path: impl AsRef<Path>) -> std::io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct Process {
    parent: u32,
    started: u64,
    ticks: u64,
    rss: u64,
}

#[cfg(target_os = "linux")]
fn process(text: &str, page_size: u64) -> Option<Process> {
    // comm is parenthesized and may itself contain spaces and parentheses.
    let (_, fields) = text.rsplit_once(')')?;
    let fields = fields.split_whitespace().take(22).collect::<Vec<_>>();
    Some(Process {
        parent: fields.get(1)?.parse().ok()?,
        started: fields.get(19)?.parse().ok()?,
        ticks: fields
            .get(11)?
            .parse::<u64>()
            .ok()?
            .checked_add(fields.get(12)?.parse().ok()?)?,
        rss: fields
            .get(21)?
            .parse::<u64>()
            .ok()?
            .checked_mul(page_size)?,
    })
}

#[cfg(target_os = "linux")]
fn descendants(
    all: &std::collections::BTreeMap<u32, Process>,
    root: u32,
) -> std::collections::BTreeMap<u32, Process> {
    all.iter()
        .filter_map(|(&pid, &entry)| {
            let mut ancestor = pid;
            // Bound even malformed/cyclic process trees. Missing/reaped parents
            // cannot be used as evidence that an unrelated process belongs to us.
            for _ in 0..128 {
                if ancestor == root {
                    return Some((pid, entry));
                }
                let parent = all.get(&ancestor)?.parent;
                if parent == ancestor || parent == 0 {
                    return None;
                }
                ancestor = parent;
            }
            None
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn observed_ticks(
    before: &std::collections::BTreeMap<u32, Process>,
    after: &std::collections::BTreeMap<u32, Process>,
) -> Option<u64> {
    after.iter().try_fold(0u64, |total, (pid, current)| {
        let Some(prior) = before
            .get(pid)
            .filter(|prior| prior.started == current.started)
        else {
            return Some(total);
        };
        total.checked_add(current.ticks.checked_sub(prior.ticks)?)
    })
}

#[cfg(target_os = "linux")]
fn processes() -> Option<std::collections::BTreeMap<u32, Process>> {
    let mut all = std::collections::BTreeMap::new();
    let page_size = u64::try_from(rustix::param::page_size()).ok()?;
    let mut entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.by_ref().take(4096).flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(entry) = bounded_read(entry.path().join("stat"))
            .ok()
            .and_then(|text| process(&text, page_size))
        {
            all.insert(pid, entry);
        }
    }
    // A partial scan must not silently under-report the process population.
    if entries.next().is_some() || !all.contains_key(&std::process::id()) {
        return None;
    }
    Some(descendants(&all, std::process::id()))
}

#[cfg(target_os = "linux")]
fn network(text: &str) -> Option<(u64, u64)> {
    let mut total = (0u64, 0u64);
    let mut found = false;
    for line in text.lines().skip(2) {
        let (name, values) = line.split_once(':')?;
        if name.trim() == "lo" {
            continue;
        }
        let values = values.split_whitespace().collect::<Vec<_>>();
        total.0 = total.0.saturating_add(values.first()?.parse::<u64>().ok()?);
        total.1 = total.1.saturating_add(values.get(8)?.parse::<u64>().ok()?);
        found = true;
    }
    found.then_some(total)
}

#[cfg(target_os = "linux")]
#[allow(clippy::cast_precision_loss)]
fn counter_rate(delta: u64, elapsed: std::time::Duration) -> f64 {
    // Network rates are approximate telemetry. Preserve the full counter range;
    // sub-byte precision is immaterial after division by the sampling interval.
    delta as f64 / elapsed.as_secs_f64()
}

#[cfg(target_os = "linux")]
fn memory_total(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == "MemTotal")
            .then(|| {
                value
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()?
                    .checked_mul(1024)
            })
            .flatten()
    })
}

#[cfg(target_os = "linux")]
fn mount_path(value: &str) -> Option<std::path::PathBuf> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let escape = bytes.get(index + 1..index + 4)?;
            let byte = match escape {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return None,
            };
            decoded.push(byte);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let path = std::path::PathBuf::from(String::from_utf8(decoded).ok()?);
    (path.is_absolute()
        && path.components().all(|part| {
            matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        }))
    .then_some(path)
}

#[cfg(target_os = "linux")]
fn memory_paths(mountinfo: &str, membership: &str) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    for line in mountinfo.lines().take(256) {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left = left.split_whitespace().collect::<Vec<_>>();
        let right = right.split_whitespace().collect::<Vec<_>>();
        let (Some(root), Some(mount)) = (
            left.get(3).and_then(|value| mount_path(value)),
            left.get(4).and_then(|value| mount_path(value)),
        ) else {
            continue;
        };
        let version = match right.first().copied() {
            Some("cgroup2") => 2,
            Some("cgroup")
                if right
                    .get(2)
                    .is_some_and(|options| options.split(',').any(|option| option == "memory")) =>
            {
                1
            }
            _ => continue,
        };
        for group in membership.lines().take(64) {
            let mut fields = group.splitn(3, ':');
            let (Some(id), Some(controllers), Some(name)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if !(version == 2 && id == "0" && controllers.is_empty()
                || version == 1
                    && controllers
                        .split(',')
                        .any(|controller| controller == "memory"))
            {
                continue;
            }
            let Some(group_path) = mount_path(name) else {
                continue;
            };
            // A namespace-relative membership '/' names the visible mount root.
            let relative = if group_path == Path::new("/") {
                Path::new("")
            } else if let Ok(relative) = group_path.strip_prefix(&root) {
                relative
            } else {
                continue;
            };
            let mut current = mount.join(relative);
            for _ in 0..128 {
                if paths.len() >= 512 {
                    return paths;
                }
                paths.push(current.join(if version == 2 {
                    "memory.max"
                } else {
                    "memory.limit_in_bytes"
                }));
                if current == mount || !current.pop() {
                    break;
                }
            }
        }
    }
    paths
}

#[cfg(target_os = "linux")]
fn memory_ceiling() -> (Option<u64>, bool) {
    let physical = bounded_read("/proc/meminfo")
        .ok()
        .and_then(|text| memory_total(&text));
    let limit = bounded_read("/proc/self/mountinfo")
        .ok()
        .zip(bounded_read("/proc/self/cgroup").ok())
        .into_iter()
        .flat_map(|(mounts, membership)| memory_paths(&mounts, &membership))
        .filter_map(|path| bounded_read(path).ok()?.trim().parse::<u64>().ok())
        .filter(|value| *value > 0 && physical.is_none_or(|total| *value < total))
        .min();
    (limit.or(physical), limit.is_some())
}

pub(super) async fn sample(data_path: &Path) -> HostMetrics {
    let result = HostMetrics {
        scope: "CPU: self CPU of daemon and descendants present in both snapshots; short-lived processes may be missed. RSS: observed descendants. Configured data filesystem; namespace network excluding loopback",
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        ..HostMetrics::default()
    };
    #[cfg(target_os = "linux")]
    {
        static ADMISSION: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
        let Ok(permit) = std::sync::Arc::clone(&ADMISSION).try_acquire_owned() else {
            return result;
        };
        let data_path = data_path.to_owned();
        return tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let before_processes = processes();
            let before_net = bounded_read("/proc/net/dev")
                .ok()
                .and_then(|text| network(&text));
            let started = std::time::Instant::now();
            std::thread::sleep(std::time::Duration::from_secs(5));
            let after_processes = processes();
            let after_net = bounded_read("/proc/net/dev")
                .ok()
                .and_then(|text| network(&text));
            let elapsed = started.elapsed();
            let cores = std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZeroUsize::get);
            let cpu_percent = before_processes
                .as_ref()
                .zip(after_processes.as_ref())
                .zip(cores)
                .and_then(|((before, after), cores)| {
                    let root = std::process::id();
                    if before.get(&root)?.started != after.get(&root)?.started {
                        return None;
                    }
                    let ticks = observed_ticks(before, after)?;
                    let ticks = u32::try_from(ticks).ok()?;
                    let ticks_per_second =
                        u32::try_from(rustix::param::clock_ticks_per_second()).ok()?;
                    let cores = u32::try_from(cores).ok()?;
                    Some(
                        (f64::from(ticks)
                            / f64::from(ticks_per_second)
                            / elapsed.as_secs_f64()
                            / f64::from(cores)
                            * 100.0)
                            .clamp(0.0, 100.0),
                    )
                });
            let memory_used = after_processes
                .as_ref()
                .map(|entries| entries.values().map(|entry| entry.rss).sum());
            let children = after_processes.as_ref().map(|entries| {
                entries
                    .iter()
                    .filter(|(pid, _)| **pid != std::process::id())
                    .map(|(_, entry)| entry.rss)
                    .collect::<Vec<_>>()
            });
            let (memory_total, memory_limit_is_cgroup) = memory_ceiling();
            let disk = rustix::fs::statvfs(&data_path).ok();
            let rates = before_net.zip(after_net).and_then(|(before, after)| {
                Some((
                    counter_rate(after.0.checked_sub(before.0)?, elapsed),
                    counter_rate(after.1.checked_sub(before.1)?, elapsed),
                ))
            });
            HostMetrics {
                available: cpu_percent.is_some()
                    || memory_used.is_some()
                    || disk.is_some()
                    || rates.is_some(),
                hostname: bounded_read("/proc/sys/kernel/hostname")
                    .ok()
                    .map(|name| name.trim().chars().take(255).collect()),
                cpu_cores: cores,
                cpu_percent,
                memory_used_bytes: memory_used,
                memory_total_bytes: memory_total,
                memory_limit_is_cgroup,
                disk_used_bytes: disk.as_ref().and_then(|value| {
                    value
                        .f_blocks
                        .saturating_sub(value.f_bfree)
                        .checked_mul(value.f_frsize)
                }),
                disk_total_bytes: disk
                    .as_ref()
                    .and_then(|value| value.f_blocks.checked_mul(value.f_frsize)),
                child_process_count: children.as_ref().map(Vec::len),
                child_rss_bytes: children.map(|rss| rss.into_iter().sum()),
                network_rx_bytes_per_second: rates.map(|value| value.0),
                network_tx_bytes_per_second: rates.map(|value| value.1),
                sample_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                ..result
            }
        })
        .await
        .unwrap_or_default();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = data_path;
        result
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn host_metrics_parse_process_identity_ticks_and_rss() {
        let entry = process(
            "42 (name with ) parens) S 7 42 42 0 0 0 0 0 0 0 100 25 0 0 20 0 3 0 999 50000 12",
            4096,
        )
        .unwrap();
        assert_eq!(entry.parent, 7);
        assert_eq!(entry.started, 999);
        assert_eq!(entry.ticks, 125);
        assert_eq!(entry.rss, 49152);
        assert!(process("42 (bad) S 7", 4096).is_none());
    }
    #[test]
    fn host_metrics_exclude_unrelated_processes_and_cycles() {
        let row = |parent| Process {
            parent,
            started: 1,
            ticks: 1,
            rss: 1,
        };
        let all = std::collections::BTreeMap::from([
            (1, row(0)),
            (10, row(1)),
            (11, row(10)),
            (12, row(11)),
            (20, row(1)),
            (30, row(31)),
            (31, row(30)),
        ]);
        assert_eq!(
            descendants(&all, 10).keys().copied().collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
    }
    #[test]
    fn host_metrics_parsers_preserve_units_and_exclude_loopback() {
        assert_eq!(
            memory_total("MemTotal: 1000 kB\nMemAvailable: 400 kB\n"),
            Some(1_024_000)
        );
        assert_eq!(
            network("header\nheader\nlo: 99 0 0 0 0 0 0 0 88\neth0: 120 0 0 0 0 0 0 0 340"),
            Some((120, 340))
        );
        assert_eq!(memory_total("MemTotal: nope"), None);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod mount_tests {
    use super::*;
    #[test]
    fn host_metrics_cpu_reuse_and_exit_never_count_historical_work() {
        let row = |started, ticks| Process {
            parent: 1,
            started,
            ticks,
            rss: 0,
        };
        let before = std::collections::BTreeMap::from([
            (1, row(1, 100)),
            (2, row(2, 500)),
            (3, row(3, 900)),
        ]);
        let after = std::collections::BTreeMap::from([
            (1, row(1, 125)),
            (2, row(99, 1000)),
            (4, row(4, 2000)),
        ]);
        assert_eq!(observed_ticks(&before, &after), Some(25));
    }
    #[test]
    fn host_metrics_cgroup_mount_roots_and_namespaces() {
        let mounts = "1 0 0:1 /tenant /sys/fs/cgroup rw - cgroup2 cgroup rw";
        assert_eq!(
            memory_paths(mounts, "0::/tenant/labby"),
            vec![
                std::path::PathBuf::from("/sys/fs/cgroup/labby/memory.max"),
                std::path::PathBuf::from("/sys/fs/cgroup/memory.max")
            ]
        );
        assert_eq!(
            memory_paths(mounts, "0::/"),
            vec![std::path::PathBuf::from("/sys/fs/cgroup/memory.max")]
        );
        assert!(memory_paths(mounts, "0::/unrelated/labby").is_empty());
    }
    #[test]
    fn host_metrics_v1_and_escaped_mounts() {
        let mounts = r"1 0 0:1 /tenant /sys/cgroup\040memory rw - cgroup memory rw,memory";
        assert_eq!(
            memory_paths(mounts, "3:cpu,memory:/tenant/labby"),
            vec![
                std::path::PathBuf::from("/sys/cgroup memory/labby/memory.limit_in_bytes"),
                std::path::PathBuf::from("/sys/cgroup memory/memory.limit_in_bytes")
            ]
        );
        assert!(mount_path(r"/bad\000path").is_none());
        assert!(mount_path("/safe/../escape").is_none());
    }
}
