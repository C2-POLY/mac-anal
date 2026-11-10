//! The Mac's three historical measures, read only: free space on the startup volume, memory,
//! processor load. The dashboard's collector, the surveillance and the MCP server read them here.

use std::path::Path;

use sysinfo::{Disks, System};

use crate::overview::{Overview, Usage};

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
