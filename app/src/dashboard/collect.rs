//! The two threads that read the Mac: a fast one every 2 s, a slow one every minute. They only
//! read, and hand each reading to a callback; the window never waits on them.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sysinfo::{Components, Disks, Gpus, Networks, System};

use super::net::Counters;
use super::sensors::{Battery, Sensors, Thermal};
use super::{cpu, hardware, net, procs, sensors, Reading, Snapshot, FAST, SLOW};

/// Longest a tool may run before it is killed: a stuck `system_profiler` must not stop the readings.
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);
/// Battery and thermal state change slowly: read them every this many fast readings (30 s).
const SENSOR_EVERY: u64 = 15;

/// What a tool printed.
pub(super) struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a tool with nothing on stdin and a time limit. `None` when it cannot start or overruns.
pub(super) fn run(program: &str, args: &[&str]) -> Option<Ran> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    // Read both pipes while the tool runs, or a large output would fill a pipe and block it.
    let pipe = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = p {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < TOOL_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    status.map(|s| Ran { ok: s.success(), stdout, stderr })
}

/// What a tool printed on stdout, when it succeeded.
pub(super) fn output(program: &str, args: &[&str]) -> Option<String> {
    run(program, args).filter(|r| r.ok).map(|r| r.stdout)
}

/// The sources of the fast readings, kept between two of them: rates and loads are differences.
struct Collector {
    sys: System,
    disks: Disks,
    networks: Networks,
    components: Components,
    gpus: Gpus,
    net_seen: Counters,
    disk_seen: Counters,
    last: Option<Instant>,
    tick: u64,
    battery: Option<Battery>,
    thermal: Option<Thermal>,
}

impl Collector {
    fn new() -> Option<Self> {
        Some(Self {
            sys: System::new().ok()?,
            disks: Disks::new_with_refreshed_list().ok()?,
            networks: Networks::new_with_refreshed_list().ok()?,
            components: Components::new_with_refreshed_list().ok()?,
            gpus: Gpus::new_with_refreshed_list().ok()?,
            net_seen: Counters::new(),
            disk_seen: Counters::new(),
            last: None,
            tick: 0,
            battery: None,
            thermal: None,
        })
    }

    fn read(&mut self) -> Snapshot {
        let first = self.last.is_none();
        let now = Instant::now();
        let elapsed = self.last.map(|l| now - l).unwrap_or(FAST);
        self.last = Some(now);
        if self.tick.is_multiple_of(SENSOR_EVERY) {
            self.battery = sensors::battery();
            self.thermal = sensors::thermal();
        }
        self.tick += 1;
        // The three historical measures, read as the menu bar and the surveillance always did.
        let overview = crate::metrics::read(&mut self.sys, &mut self.disks, first);
        Snapshot {
            at: crate::journal::now(),
            overview,
            identity: cpu::identity(),
            cpu: cpu::cpu(&self.sys, first),
            memory: cpu::memory(&self.sys),
            gpus: cpu::gpus(&mut self.gpus),
            sensors: Sensors {
                temperatures: sensors::temperatures(&mut self.components),
                fans: sensors::fans(),
                thermal: self.thermal,
                battery: self.battery.clone(),
            },
            interfaces: net::interfaces(&mut self.networks, &mut self.net_seen, elapsed),
            volumes: net::volumes(&self.disks, &mut self.disk_seen, elapsed),
            processes: Some(procs::read(&mut self.sys, elapsed)).filter(|p| p.count > 0),
        }
    }
}

/// Starts both threads. `on_reading` is called from them, with each reading.
pub fn spawn(on_reading: impl Fn(Reading) + Send + Sync + 'static) {
    let on_reading = Arc::new(on_reading);
    let fast = on_reading.clone();
    std::thread::Builder::new()
        .name("dashboard-fast".into())
        .spawn(move || {
            let Some(mut c) = Collector::new() else { return };
            loop {
                let start = Instant::now();
                fast(Reading::Fast(Box::new(c.read())));
                std::thread::sleep(FAST.saturating_sub(start.elapsed()));
            }
        })
        .expect("start the dashboard thread");
    std::thread::Builder::new()
        .name("dashboard-slow".into())
        .spawn(move || loop {
            on_reading(Reading::Slow(hardware::read()));
            std::thread::sleep(SLOW);
        })
        .expect("start the hardware thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_s_output_and_errors_are_kept() {
        let r = run("/bin/sh", &["-c", "echo out; echo err >&2; exit 3"]).unwrap();
        assert!(!r.ok);
        assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("out\n", "err\n"));
        assert_eq!(output("/bin/echo", &["bonjour"]).as_deref(), Some("bonjour\n"));
        assert!(output("/bin/sh", &["-c", "exit 1"]).is_none());
        assert!(run("/no/such/tool", &[]).is_none());
    }

    #[test]
    fn a_large_output_does_not_block_the_tool() {
        let r = run("/bin/sh", &["-c", "head -c 1000000 /dev/zero"]).unwrap();
        assert_eq!(r.stdout.len(), 1_000_000);
    }

    #[test]
    fn a_full_reading_of_this_mac_does_not_panic() {
        let mut c = Collector::new().expect("sources");
        let first = c.read();
        assert!(first.cpu.is_none() && first.overview.cpu.is_none(), "the load needs two readings");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let s = c.read();
        assert!(s.at > 0);
        assert!(s.identity.is_some() && s.cpu.is_some() && s.memory.is_some() && s.processes.is_some());
        assert!(s.overview.disk.is_some() && s.overview.memory.is_some() && s.overview.cpu.is_some());
        assert!(!s.volumes.is_empty());
    }
}
