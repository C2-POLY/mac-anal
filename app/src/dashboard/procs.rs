//! The processes that weigh the most, and how many processes, threads and open files the Mac has.

use std::time::Duration;

use sysinfo::{ProcessesToUpdate, System};

use super::cpu::sysctl_u64;
use super::net::rate;

/// How many processes the list shows.
pub const TOP: usize = 15;

/// One process.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    /// Load as `top` counts it: 1 is one full core, so it can exceed 1.
    pub cpu: f32,
    /// Resident memory, in bytes.
    pub memory: u64,
    /// Bytes read and written per second.
    pub disk_per_s: u64,
}

/// The column the list is sorted by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Cpu,
    Memory,
    Disk,
}

/// The processes of a reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Processes {
    /// Every process, unsorted; the window sorts and cuts it to `TOP`.
    pub all: Vec<Process>,
    pub count: usize,
    pub threads: Option<u64>,
    pub open_files: Option<u64>,
}

impl Processes {
    /// The `TOP` heaviest processes for that column, heaviest first.
    pub fn top(&self, by: SortBy) -> Vec<&Process> {
        let mut list: Vec<&Process> = self.all.iter().collect();
        match by {
            SortBy::Cpu => list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            SortBy::Memory => list.sort_by_key(|p| std::cmp::Reverse(p.memory)),
            SortBy::Disk => list.sort_by_key(|p| std::cmp::Reverse(p.disk_per_s)),
        }
        list.truncate(TOP);
        list
    }

    /// Invented values for the screenshot mode.
    pub fn sample() -> Self {
        const MB: u64 = 1_000_000;
        let p = |pid, name: &str, cpu, memory, disk_per_s| Process { pid, name: name.into(), cpu, memory, disk_per_s };
        let all = vec![
            p(812, "Safari", 0.42, 1_850 * MB, 120_000),
            p(1_204, "Xcode", 1.37, 3_200 * MB, 4_800_000),
            p(97, "WindowServer", 0.21, 640 * MB, 0),
            p(2_311, "Code Helper (Renderer)", 0.18, 910 * MB, 20_000),
            p(388, "mds_stores", 0.12, 210 * MB, 9_600_000),
            p(1_590, "Mail", 0.03, 420 * MB, 0),
            p(455, "kernel_task", 0.09, 1_100 * MB, 300_000),
            p(1_877, "Spotify", 0.05, 380 * MB, 50_000),
        ];
        Self { count: 612, threads: Some(3_418), open_files: Some(45_967), all }
    }
}

/// Every process, with the load and the disk activity since the previous refresh.
pub fn read(sys: &mut System, elapsed: Duration) -> Processes {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let all: Vec<Process> = sys
        .processes()
        .values()
        .map(|p| {
            let io = p.disk_usage();
            Process {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().into_owned(),
                cpu: p.cpu_usage() / 100.0,
                memory: p.memory(),
                disk_per_s: rate(io.read_bytes + io.written_bytes, elapsed),
            }
        })
        .collect();
    Processes { count: all.len(), threads: sysctl_u64("kern.num_threads"), open_files: sysctl_u64("kern.num_files"), all }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn many(n: u32) -> Processes {
        let all = (0..n).map(|i| Process { pid: i, name: format!("p{i}"), cpu: i as f32 / 10.0, memory: (n - i) as u64, disk_per_s: (i % 3) as u64 }).collect();
        Processes { all, count: n as usize, threads: None, open_files: None }
    }

    #[test]
    fn the_top_is_sorted_by_the_chosen_column_and_cut() {
        let p = many(40);
        let cpu = p.top(SortBy::Cpu);
        assert_eq!(cpu.len(), TOP);
        assert_eq!(cpu[0].pid, 39);
        assert!(cpu.windows(2).all(|w| w[0].cpu >= w[1].cpu));
        let mem = p.top(SortBy::Memory);
        assert_eq!(mem[0].pid, 0);
        assert!(mem.windows(2).all(|w| w[0].memory >= w[1].memory));
        let disk = p.top(SortBy::Disk);
        assert_eq!(disk[0].disk_per_s, 2);
        assert!(disk.windows(2).all(|w| w[0].disk_per_s >= w[1].disk_per_s));
    }

    #[test]
    fn a_short_list_is_kept_whole() {
        assert_eq!(many(4).top(SortBy::Cpu).len(), 4);
        assert!(Processes::default().top(SortBy::Memory).is_empty());
    }

    #[test]
    fn this_mac_s_processes_are_counted() {
        let mut sys = System::new().unwrap();
        let p = read(&mut sys, Duration::from_secs(2));
        assert!(p.count > 10 && p.count == p.all.len());
        assert!(p.threads.unwrap() >= p.count as u64);
        assert!(p.open_files.unwrap() > 0);
        assert!(p.all.iter().any(|q| q.pid == std::process::id()));
    }
}
