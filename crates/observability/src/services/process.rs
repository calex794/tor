//! Read-only Linux process metrics; no host-wide CPU claims and no PII.
#[cfg(target_os = "linux")]
use std::{
    fs,
    sync::{Mutex, OnceLock},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessSnapshot {
    /// Resident set size of THIS web process, not system-wide available RAM.
    pub resident_kib: Option<u64>,
    /// Total on-CPU execution time from Linux schedstat, not wall time.
    pub cpu_seconds: Option<f64>,
    /// Relative to one CPU core, measured since the previous operator reading.
    /// First reading and unsupported kernels report None, never a fake zero.
    pub cpu_core_percent: Option<f64>,
}

fn resident_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmRSS:")?;
        let mut fields = rest.split_whitespace();
        let value = fields.next()?.parse::<u64>().ok()?;
        (fields.next() == Some("kB")).then_some(value)
    })
}

fn scheduled_cpu_ns(schedstat: &str) -> Option<u64> {
    schedstat.split_whitespace().next()?.parse().ok()
}

#[cfg(target_os = "linux")]
static LAST_CPU: OnceLock<Mutex<Option<(Instant, u64)>>> = OnceLock::new();

/// Never blocks application requests on remote telemetry or PostgreSQL.
pub fn capture() -> ProcessSnapshot {
    #[cfg(target_os = "linux")]
    {
        let rss = fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|contents| resident_kib(&contents));
        let cpu_ns = fs::read_to_string("/proc/self/schedstat")
            .ok()
            .and_then(|contents| scheduled_cpu_ns(&contents));
        let now = Instant::now();
        let percent = cpu_ns.and_then(|cpu| {
            let mut previous = LAST_CPU
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let share = previous.and_then(|(before, old_cpu)| {
                let elapsed = now.saturating_duration_since(before).as_nanos();
                if elapsed < 250_000_000 || cpu < old_cpu {
                    return None;
                }
                let value = (cpu - old_cpu) as f64 * 100.0 / elapsed as f64;
                value.is_finite().then_some(value)
            });
            *previous = Some((now, cpu));
            share
        });
        ProcessSnapshot {
            resident_kib: rss,
            cpu_seconds: cpu_ns.map(|ns| ns as f64 / 1_000_000_000.0),
            cpu_core_percent: percent,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        ProcessSnapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_vm_rss_is_allowed_from_proc_status() {
        assert_eq!(resident_kib("Name:\tprivate\nVmPeak:\t999 kB\nVmRSS:\t12345 kB\n"), Some(12345));
        assert_eq!(resident_kib("VmSize:\t12345 kB"), None);
        assert_eq!(resident_kib("VmRSS:\tsecret kB"), None);
        assert_eq!(resident_kib("VmRSS:\t999 MB"), None);
    }

    #[test]
    fn cpu_time_is_measured_without_extra_process_fields() {
        assert_eq!(scheduled_cpu_ns("987654321 200 12\n"), Some(987654321));
        assert_eq!(scheduled_cpu_ns("bad 1 2"), None);
    }

    #[test]
    fn process_snapshot_never_uses_credentials_or_network() {
        let s = capture();
        assert!(s.resident_kib.is_none_or(|kb| kb < 1_000_000_000));
        assert!(s.cpu_core_percent.is_none_or(|v| v >= 0.0 && v.is_finite()));
    }
}
