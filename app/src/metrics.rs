//! The Mac's measures, read only: free space on the startup volume, memory, processor load. A
//! thread refreshes them every few seconds and hands each reading to a callback.

use std::path::Path;
use std::time::Duration;

use sysinfo::{Disks, System};

use crate::overview::{Overview, Usage};

/// Time between two readings: cheap enough to run all day, fresh enough for the menu bar.
pub const INTERVAL: Duration = Duration::from_secs(5);

/// One reading. The processor load needs two readings apart, so the first one has none.
pub fn read(sys: &mut System, disks: &mut Disks, first: bool) -> Overview {
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    disks.refresh(true);
    let disk = disks.iter().find(|d| d.mount_point() == Path::new("/")).map(|d| Usage {
        used: d.total_space().saturating_sub(d.available_space()),
        total: d.total_space(),
    });
    let memory = Some(Usage { used: sys.used_memory(), total: sys.total_memory() }).filter(|u| u.total > 0);
    let cpu = (!first).then(|| (sys.global_cpu_usage() / 100.0).clamp(0.0, 1.0));
    Overview { disk, memory, cpu }
}

/// Reads forever on its own thread and calls `on_reading` with each reading.
pub fn spawn(on_reading: impl Fn(Overview) + Send + 'static) {
    std::thread::Builder::new()
        .name("metrics".into())
        .spawn(move || {
            let Ok(mut sys) = System::new() else { return };
            let Ok(mut disks) = Disks::new_with_refreshed_list() else { return };
            let mut first = true;
            loop {
                on_reading(read(&mut sys, &mut disks, first));
                first = false;
                std::thread::sleep(INTERVAL);
            }
        })
        .expect("start the metrics thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_of_this_mac_is_consistent() {
        let mut sys = System::new().unwrap();
        let mut disks = Disks::new_with_refreshed_list().unwrap();
        let first = read(&mut sys, &mut disks, true);
        assert!(first.cpu.is_none());
        let disk = first.disk.expect("the startup volume");
        assert!(disk.total > 0 && disk.used <= disk.total);
        let mem = first.memory.expect("memory");
        assert!(mem.total > 0 && mem.used <= mem.total);

        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let cpu = read(&mut sys, &mut disks, false).cpu.unwrap();
        assert!((0.0..=1.0).contains(&cpu));
    }
}
