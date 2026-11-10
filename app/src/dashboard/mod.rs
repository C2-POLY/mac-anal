//! The dashboard: everything the Mac tells without administrator rights, read on a thread and
//! drawn in the window. A section whose source is missing stays `None` (or empty) and shows
//! « indisponible »; nothing here ever panics on a Mac that lacks a sensor, a battery or a fan.

mod collect;
mod cpu;
pub mod format;
mod hardware;
mod history;
mod net;
mod procs;
mod sensors;
mod ui;

use std::sync::mpsc::Receiver;

pub use collect::spawn;
pub use cpu::{Cpu, Gpu, Identity, Memory};
pub use hardware::Hardware;
pub use history::History;
pub use net::{Interface, Volume};
pub use procs::{Processes, SortBy};
pub use sensors::{Sensors, Thermal};

use crate::overview::Overview;

/// Time between two fast readings: processor, memory, network, disks, processes, sensors.
pub const FAST: std::time::Duration = std::time::Duration::from_secs(2);
/// Time between two slow readings: `system_profiler` and `tmutil` take seconds.
pub const SLOW: std::time::Duration = std::time::Duration::from_secs(60);

/// One fast reading of the Mac.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// When it was read, in seconds since 1970.
    pub at: u64,
    /// The three measures the menu bar, the surveillance and the MCP server already use.
    pub overview: Overview,
    pub identity: Option<Identity>,
    pub cpu: Option<Cpu>,
    pub memory: Option<Memory>,
    pub gpus: Vec<Gpu>,
    pub sensors: Sensors,
    pub interfaces: Vec<Interface>,
    pub volumes: Vec<Volume>,
    pub processes: Option<Processes>,
}

impl Snapshot {
    /// Network throughput, all interfaces together, in bytes per second (received, sent).
    pub fn network_rate(&self) -> (u64, u64) {
        self.interfaces.iter().fold((0, 0), |(r, s), i| (r + i.received_per_s, s + i.sent_per_s))
    }

    /// Disk throughput, all volumes together, in bytes per second (read, written).
    pub fn disk_rate(&self) -> (u64, u64) {
        self.volumes.iter().fold((0, 0), |(r, w), v| (r + v.read_per_s, w + v.written_per_s))
    }
}

/// What the collector sends: a fast reading every 2 s, a slow one every minute.
pub enum Reading {
    Fast(Box<Snapshot>),
    Slow(Hardware),
}

/// The series kept for the curves, one per measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Series {
    Cpu,
    Memory,
    Swap,
    Gpu,
    NetIn,
    NetOut,
    DiskRead,
    DiskWrite,
    Temperature,
}

/// The dashboard as the window holds it: the last readings, their history and the choices made.
pub struct Dashboard {
    pub snapshot: Snapshot,
    pub hardware: Option<Hardware>,
    pub history: History,
    pub sort: SortBy,
    readings: Option<Receiver<Reading>>,
}

impl Dashboard {
    /// A dashboard fed by the collector's readings.
    pub fn live(readings: Receiver<Reading>) -> Self {
        Self { snapshot: Snapshot::default(), hardware: None, history: History::default(), sort: SortBy::Cpu, readings: Some(readings) }
    }

    /// Invented values for the screenshot mode: nothing is read on the Mac.
    pub fn sample() -> Self {
        let snapshot = Snapshot::sample();
        let history = History::sample(snapshot.at);
        Self { snapshot, hardware: Some(Hardware::sample()), history, sort: SortBy::Cpu, readings: None }
    }

    /// Takes the readings that arrived since the last frame. Returns the newest overview, if any.
    pub fn poll(&mut self) -> Option<Overview> {
        let mut fresh = None;
        let readings: Vec<Reading> = self.readings.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for r in readings {
            match r {
                Reading::Fast(s) => {
                    self.history.record(&s);
                    fresh = Some(s.overview);
                    self.snapshot = *s;
                }
                Reading::Slow(h) => self.hardware = Some(h),
            }
        }
        fresh
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui::show(self, ui);
    }

    pub fn show_processes(&mut self, ui: &mut egui::Ui) {
        ui::show_processes(self, ui);
    }
}

const GB: u64 = 1_000_000_000;
const MB: u64 = 1_000_000;

impl Snapshot {
    /// Invented values for the screenshot mode.
    pub fn sample() -> Self {
        let at = 1_794_211_200; // 2026-11-09, 10:00 UTC
        Self {
            at,
            overview: Overview::sample(),
            identity: Some(Identity {
                model: "Mac mini".into(),
                model_id: "Mac16,11".into(),
                chip: "Apple M4 Pro".into(),
                os: "macOS 26.6.2 (25G83)".into(),
                kernel: "25.6.0".into(),
                host: "mac-mini".into(),
                uptime: 3 * 86_400 + 4 * 3_600 + 12 * 60,
            }),
            cpu: Some(Cpu {
                cores: vec![0.62, 0.48, 0.35, 0.22, 0.18, 0.12, 0.09, 0.07, 0.05, 0.04, 0.41, 0.33, 0.28, 0.19],
                performance: 10,
                efficiency: 4,
                load: [3.42, 2.87, 2.51],
                frequency_mhz: 4_512,
            }),
            memory: Some(Memory {
                total: 16 * GB,
                app: 7_200 * MB,
                wired: 2_100 * MB,
                compressed: 1_700 * MB,
                cached: 3_400 * MB,
                free: 1_600 * MB,
                swap_used: 1_200 * MB,
                swap_total: 2 * GB,
                pressure_free: Some(0.44),
            }),
            gpus: vec![Gpu { name: "Apple M4 Pro".into(), usage: Some(0.31) }],
            sensors: Sensors::sample(),
            interfaces: vec![
                Interface { name: "en0".into(), address: Some("192.168.1.24".into()), received_per_s: 2_400_000, sent_per_s: 310_000, received_total: 41 * GB, sent_total: 6 * GB },
                Interface { name: "utun4".into(), address: Some("100.64.0.3".into()), received_per_s: 12_000, sent_per_s: 9_000, received_total: 900 * MB, sent_total: 400 * MB },
            ],
            volumes: vec![
                Volume { name: "Macintosh HD".into(), mount: "/".into(), file_system: "apfs".into(), used: 312 * GB, total: 494 * GB, removable: false, read_per_s: 18 * MB, written_per_s: 4 * MB },
                Volume { name: "Sauvegardes".into(), mount: "/Volumes/Sauvegardes".into(), file_system: "apfs".into(), used: 1_400 * GB, total: 2_000 * GB, removable: true, read_per_s: 0, written_per_s: 0 },
            ],
            processes: Some(Processes::sample()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_fills_every_section() {
        let d = Dashboard::sample();
        let s = &d.snapshot;
        assert!(s.identity.is_some() && s.cpu.is_some() && s.memory.is_some() && s.processes.is_some());
        assert!(!s.gpus.is_empty() && !s.interfaces.is_empty() && !s.volumes.is_empty());
        assert!(!s.sensors.temperatures.is_empty() && !s.sensors.fans.is_empty() && s.sensors.battery.is_some());
        let h = d.hardware.as_ref().unwrap();
        assert!(!h.displays.is_empty() && !h.usb.is_empty() && !h.bluetooth.is_empty());
        assert!(d.history.points(Series::Cpu).len() > 100);
    }

    #[test]
    fn an_empty_snapshot_has_every_section_missing() {
        let s = Snapshot::default();
        assert!(s.identity.is_none() && s.cpu.is_none() && s.memory.is_none() && s.processes.is_none());
        assert!(s.gpus.is_empty() && s.interfaces.is_empty() && s.volumes.is_empty());
        assert_eq!(s.network_rate(), (0, 0));
    }

    #[test]
    fn rates_add_every_interface_and_volume() {
        let s = Snapshot::sample();
        assert_eq!(s.network_rate(), (2_412_000, 319_000));
        assert_eq!(s.disk_rate(), (18 * MB, 4 * MB));
    }

    #[test]
    fn readings_reach_the_dashboard_and_its_history() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut d = Dashboard::live(rx);
        assert!(d.poll().is_none());
        tx.send(Reading::Fast(Box::new(Snapshot::sample()))).unwrap();
        tx.send(Reading::Slow(Hardware::sample())).unwrap();
        assert_eq!(d.poll(), Some(Overview::sample()));
        assert!(d.snapshot.cpu.is_some() && d.hardware.is_some());
        assert_eq!(d.history.points(Series::Cpu).len(), 1);
    }
}
