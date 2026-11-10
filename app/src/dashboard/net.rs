//! The network interfaces and the mounted volumes, with their throughput. `sysinfo` counts the
//! bytes since the previous refresh; the rate divides them by the time between the two readings.

use std::collections::HashMap;
use std::time::Duration;

use sysinfo::{Disks, Networks};

/// One network interface that carries traffic.
#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub name: String,
    /// Its first IPv4 address, else its first IPv6 one.
    pub address: Option<String>,
    pub received_per_s: u64,
    pub sent_per_s: u64,
    /// Bytes since the Mac started.
    pub received_total: u64,
    pub sent_total: u64,
}

/// One mounted volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub name: String,
    pub mount: String,
    pub file_system: String,
    pub used: u64,
    pub total: u64,
    pub removable: bool,
    pub read_per_s: u64,
    pub written_per_s: u64,
}

/// Bytes counted over `elapsed`, as bytes per second.
pub fn rate(bytes: u64, elapsed: Duration) -> u64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        0
    } else {
        (bytes as f64 / secs).round() as u64
    }
}

/// Bytes between two readings of a counter that only grows, or zero when it went back (an
/// interface reset, a volume mounted again): never a negative or huge rate.
pub fn delta(previous: u64, now: u64) -> u64 {
    now.saturating_sub(previous)
}

/// The totals of the previous reading, by interface name or mount point: (in, out) or (read, written).
pub type Counters = HashMap<String, (u64, u64)>;

/// The interfaces that are up and have carried something, busiest first. Loopback is left out.
pub fn interfaces(networks: &mut Networks, previous: &mut Counters, elapsed: Duration) -> Vec<Interface> {
    networks.refresh(true);
    let mut list: Vec<Interface> = networks
        .iter()
        .filter(|(name, data)| !name.to_string_lossy().starts_with("lo") && data.total_received() + data.total_transmitted() > 0)
        .map(|(name, data)| {
            let name = name.to_string_lossy().into_owned();
            let ips = data.ip_networks();
            let address = ips.iter().find(|ip| ip.addr.is_ipv4()).or(ips.first()).map(|ip| ip.addr.to_string());
            let now = (data.total_received(), data.total_transmitted());
            // A new interface has no previous reading: its first rate is zero, not its whole total.
            let before = previous.insert(name.clone(), now).unwrap_or(now);
            Interface {
                name,
                address,
                received_per_s: rate(delta(before.0, now.0), elapsed),
                sent_per_s: rate(delta(before.1, now.1), elapsed),
                received_total: data.total_received(),
                sent_total: data.total_transmitted(),
            }
        })
        .collect();
    list.sort_by(|a, b| (b.received_per_s + b.sent_per_s).cmp(&(a.received_per_s + a.sent_per_s)).then(b.received_total.cmp(&a.received_total)));
    list
}

/// Every mounted volume but the system's own hidden ones (`/System/Volumes/…`), startup volume
/// first. `disks` was refreshed by the caller, with the overview.
pub fn volumes(disks: &Disks, previous: &mut Counters, elapsed: Duration) -> Vec<Volume> {
    let mut list: Vec<Volume> = disks
        .iter()
        .filter(|d| {
            let m = d.mount_point();
            m == std::path::Path::new("/") || !m.starts_with("/System/Volumes")
        })
        .map(|d| {
            let io = d.usage();
            let mount = d.mount_point().display().to_string();
            let now = (io.total_read_bytes, io.total_written_bytes);
            let before = previous.insert(mount.clone(), now).unwrap_or(now);
            Volume {
                name: d.name().to_string_lossy().into_owned(),
                mount,
                file_system: d.file_system().to_string_lossy().into_owned(),
                used: d.total_space().saturating_sub(d.available_space()),
                total: d.total_space(),
                removable: d.is_removable(),
                read_per_s: rate(delta(before.0, now.0), elapsed),
                written_per_s: rate(delta(before.1, now.1), elapsed),
            }
        })
        .collect();
    list.sort_by_key(|v| (v.mount != "/", v.mount.clone()));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_divides_by_the_time_between_two_readings() {
        assert_eq!(rate(4_000, Duration::from_secs(2)), 2_000);
        assert_eq!(rate(1_000, Duration::from_millis(500)), 2_000);
        assert_eq!(rate(1_000, Duration::ZERO), 0);
    }

    #[test]
    fn a_counter_that_went_back_gives_zero_not_a_negative_rate() {
        assert_eq!(delta(1_000, 1_500), 500);
        assert_eq!(delta(1_500, 200), 0);
        assert_eq!(rate(delta(u64::MAX, 3), Duration::from_secs(2)), 0);
    }

    #[test]
    fn this_mac_s_volumes_and_interfaces_are_consistent() {
        let disks = Disks::new_with_refreshed_list().unwrap();
        let mut seen = Counters::new();
        let vols = volumes(&disks, &mut seen, Duration::from_secs(2));
        assert!(vols.iter().all(|v| v.read_per_s == 0 && v.written_per_s == 0), "a first reading has no rate");
        assert_eq!(vols.first().map(|v| v.mount.as_str()), Some("/"));
        assert!(vols.iter().all(|v| v.used <= v.total));

        let mut nets = Networks::new_with_refreshed_list().unwrap();
        let ifs = interfaces(&mut nets, &mut Counters::new(), Duration::from_secs(2));
        assert!(ifs.iter().all(|i| !i.name.starts_with("lo")));
    }
}
