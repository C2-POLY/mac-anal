//! The processor, the memory, the GPU and who the Mac is: `sysinfo`, `sysctl` and the Mach VM
//! statistics, the same counters Activity Monitor shows.

use std::ffi::CString;

use sysinfo::{Gpus, Product, System};

/// What the Mac is and how long it has been up.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// « Mac mini », « MacBook Pro ».
    pub model: String,
    /// `hw.model`, for example `Mac16,11`.
    pub model_id: String,
    pub chip: String,
    pub os: String,
    pub kernel: String,
    pub host: String,
    /// Seconds since the last start.
    pub uptime: u64,
}

/// The processor load, core by core.
#[derive(Clone, Debug, PartialEq)]
pub struct Cpu {
    /// Load of each core between 0 and 1, efficiency cores first (that is how Apple numbers them).
    pub cores: Vec<f32>,
    pub performance: usize,
    pub efficiency: usize,
    /// Load average over 1, 5 and 15 minutes.
    pub load: [f64; 3],
    pub frequency_mhz: u64,
}

impl Cpu {
    /// Whether the core at `index` is an efficiency core.
    pub fn is_efficiency(&self, index: usize) -> bool {
        index < self.efficiency
    }
}

/// The memory, split as Activity Monitor does. All in bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    pub total: u64,
    /// Memory the applications hold (anonymous pages, minus what can be purged).
    pub app: u64,
    /// Memory the kernel keeps in place.
    pub wired: u64,
    /// Memory squeezed by the compressor.
    pub compressed: u64,
    /// File cache, given back as soon as an application needs it.
    pub cached: u64,
    pub free: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// Share of memory the system considers free (`kern.memorystatus_level`): low means pressure.
    pub pressure_free: Option<f32>,
}

impl Memory {
    /// What is really in use: everything but the cache and the free pages.
    pub fn used(&self) -> u64 {
        self.app + self.wired + self.compressed
    }
}

/// One graphics processor.
#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub name: String,
    /// Between 0 and 1.
    pub usage: Option<f32>,
}

/// The page counters this module reads from `host_statistics64`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pages {
    pub free: u64,
    pub wired: u64,
    pub internal: u64,
    pub external: u64,
    pub purgeable: u64,
    pub compressor: u64,
}

impl Memory {
    /// The split of Activity Monitor, from page counts.
    pub fn from_pages(p: Pages, page_size: u64, total: u64, swap: (u64, u64), pressure_free: Option<f32>) -> Self {
        Self {
            total,
            app: p.internal.saturating_sub(p.purgeable) * page_size,
            wired: p.wired * page_size,
            compressed: p.compressor * page_size,
            cached: (p.external + p.purgeable) * page_size,
            free: p.free * page_size,
            swap_used: swap.0,
            swap_total: swap.1,
            pressure_free,
        }
    }
}

/// A `sysctl` value as raw bytes.
fn sysctl_raw(name: &str) -> Option<Vec<u8>> {
    let name = CString::new(name).ok()?;
    let mut len = 0usize;
    // SAFETY: a null buffer asks for the size; the name is a valid C string.
    if unsafe { libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) } != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: the buffer holds `len` bytes, the size the kernel just gave.
    if unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

/// A string `sysctl`, without its final NUL.
pub fn sysctl_string(name: &str) -> Option<String> {
    let raw = sysctl_raw(name)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Some(String::from_utf8_lossy(&raw[..end]).trim().to_string()).filter(|s| !s.is_empty())
}

/// An integer `sysctl`, 32 or 64 bits.
pub fn sysctl_u64(name: &str) -> Option<u64> {
    let raw = sysctl_raw(name)?;
    match raw.len() {
        4 => Some(u32::from_ne_bytes(raw.try_into().ok()?) as u64),
        8 => Some(u64::from_ne_bytes(raw.try_into().ok()?)),
        _ => None,
    }
}

pub fn identity() -> Option<Identity> {
    let model_id = sysctl_string("hw.model")?;
    Some(Identity {
        model: Product::name().unwrap_or_else(|_| model_id.clone()),
        model_id,
        chip: sysctl_string("machdep.cpu.brand_string").unwrap_or_default(),
        os: System::long_os_version().unwrap_or_default(),
        kernel: System::kernel_version().unwrap_or_default(),
        host: System::host_name().unwrap_or_default(),
        uptime: System::uptime().unwrap_or(0),
    })
}

/// The load core by core. The first reading has no load yet: it needs two readings apart.
pub fn cpu(sys: &System, first: bool) -> Option<Cpu> {
    if first || sys.cpus().is_empty() {
        return None;
    }
    let cores: Vec<f32> = sys.cpus().iter().map(|c| (c.usage() / 100.0).clamp(0.0, 1.0)).collect();
    let efficiency = sysctl_u64("hw.perflevel1.logicalcpu").unwrap_or(0) as usize;
    let efficiency = efficiency.min(cores.len());
    let load = System::load_average().map(|l| [l.one, l.five, l.fifteen]).unwrap_or_default();
    Some(Cpu {
        performance: cores.len() - efficiency,
        efficiency,
        frequency_mhz: sys.cpus().iter().map(|c| c.frequency()).max().unwrap_or(0),
        cores,
        load,
    })
}

/// The page counters of the Mach VM.
fn pages() -> Option<Pages> {
    // SAFETY: a zeroed `vm_statistics64` is valid; the kernel fills at most `count` integers of it.
    unsafe {
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let r = libc::host_statistics64(host, libc::HOST_VM_INFO64, (&mut stats as *mut libc::vm_statistics64).cast(), &mut count);
        (r == libc::KERN_SUCCESS).then_some(Pages {
            free: stats.free_count as u64,
            wired: stats.wire_count as u64,
            internal: stats.internal_page_count as u64,
            external: stats.external_page_count as u64,
            purgeable: stats.purgeable_count as u64,
            compressor: stats.compressor_page_count as u64,
        })
    }
}

pub fn memory(sys: &System) -> Option<Memory> {
    let total = sys.total_memory();
    if total == 0 {
        return None;
    }
    let page_size = sysctl_u64("hw.pagesize").filter(|&p| p > 0)?;
    let pressure = sysctl_u64("kern.memorystatus_level").map(|l| (l as f32 / 100.0).clamp(0.0, 1.0));
    Some(Memory::from_pages(pages()?, page_size, total, (sys.used_swap(), sys.total_swap()), pressure))
}

pub fn gpus(gpus: &mut Gpus) -> Vec<Gpu> {
    gpus.refresh(true);
    gpus.iter()
        .map(|g| Gpu { name: g.model().or(g.vendor()).unwrap_or("GPU").to_string(), usage: g.usage().map(|u| (u / 100.0).clamp(0.0, 1.0)) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_split_as_activity_monitor_does() {
        let p = Pages { free: 10, wired: 20, internal: 50, external: 30, purgeable: 5, compressor: 15 };
        let m = Memory::from_pages(p, 16_384, 200 * 16_384, (1, 2), Some(0.4));
        assert_eq!(m.app, 45 * 16_384);
        assert_eq!(m.cached, 35 * 16_384);
        assert_eq!(m.used(), (45 + 20 + 15) * 16_384);
        assert_eq!((m.swap_used, m.swap_total), (1, 2));
    }

    #[test]
    fn purgeable_pages_never_make_app_memory_negative() {
        let p = Pages { internal: 3, purgeable: 9, ..Pages::default() };
        assert_eq!(Memory::from_pages(p, 4096, 1, (0, 0), None).app, 0);
    }

    #[test]
    fn efficiency_cores_come_first() {
        let c = Cpu { cores: vec![0.0; 6], performance: 4, efficiency: 2, load: [0.0; 3], frequency_mhz: 0 };
        assert!(c.is_efficiency(0) && c.is_efficiency(1) && !c.is_efficiency(2));
    }

    #[test]
    fn this_mac_is_read_consistently() {
        let id = identity().expect("hw.model");
        assert!(!id.model_id.is_empty() && id.uptime > 0);
        assert!(sysctl_string("no.such.sysctl").is_none());

        let mut sys = System::new().unwrap();
        sys.refresh_memory();
        let m = memory(&sys).expect("memory");
        assert!(m.used() <= m.total, "{m:?}");
        assert!(m.swap_used <= m.swap_total.max(m.swap_used));
        if let Some(p) = m.pressure_free {
            assert!((0.0..=1.0).contains(&p));
        }

        sys.refresh_cpu_usage();
        assert!(cpu(&sys, true).is_none());
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        let c = cpu(&sys, false).expect("cores");
        assert_eq!(c.performance + c.efficiency, c.cores.len());
        assert!(c.cores.iter().all(|l| (0.0..=1.0).contains(l)));
        assert!(c.load.iter().all(|l| *l >= 0.0));

        let mut g = Gpus::new().unwrap();
        for gpu in gpus(&mut g) {
            assert!(gpu.usage.is_none_or(|u| (0.0..=1.0).contains(&u)));
        }
    }
}
