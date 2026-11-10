//! Temperatures, fans, thermal throttling and the battery: `sysinfo` for the temperature
//! sensors, the System Management Controller through IOKit for the fans, `pmset -g therm` for
//! the speed limit and `ioreg` for the battery. None of it needs administrator rights, and a Mac
//! without a fan, a battery or a sensor simply gets an empty list or `None`.

use std::ffi::{c_char, c_void};

/// What the sensors read at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sensors {
    /// (label, °C), hottest first.
    pub temperatures: Vec<(String, f32)>,
    pub fans: Vec<Fan>,
    pub thermal: Option<Thermal>,
    pub battery: Option<Battery>,
}

/// One fan, in revolutions per minute.
#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub rpm: f32,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

/// Whether the system slows the processor down to cool it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Thermal {
    Nominal,
    /// CPU speed limit in percent, below 100.
    Throttled(u8),
}

/// The battery, as the power manager sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    /// Charge between 0 and 1.
    pub charge: f32,
    pub cycles: u32,
    /// Raw maximum capacity over design capacity: 1 when new, a little above happens.
    pub health: Option<f32>,
    pub charging: bool,
    /// Plugged into a power adapter.
    pub external: bool,
    /// °C.
    pub temperature: Option<f32>,
    /// Minutes until empty (or full when charging); `None` while the system has no estimate.
    pub minutes_left: Option<u32>,
}

impl Sensors {
    /// Invented values for the screenshot mode.
    pub fn sample() -> Self {
        Self {
            temperatures: vec![
                ("PMU tdie4".into(), 58.4),
                ("PMU tdie1".into(), 55.1),
                ("PMU tdev2".into(), 47.9),
                ("NAND CH0 temp".into(), 41.0),
                ("PMU tcal".into(), 36.2),
            ],
            fans: vec![Fan { rpm: 1_210.0, min: Some(1_000.0), max: Some(4_900.0) }],
            thermal: Some(Thermal::Nominal),
            battery: Some(Battery {
                charge: 0.82,
                cycles: 214,
                health: Some(0.93),
                charging: true,
                external: true,
                temperature: Some(30.5),
                minutes_left: Some(47),
            }),
        }
    }
}

/// The temperature sensors `sysinfo` knows, refreshed, hottest first. Sensors without a reading
/// (or with a nonsense one) are left out.
pub fn temperatures(components: &mut sysinfo::Components) -> Vec<(String, f32)> {
    components.refresh(true);
    let mut temps: Vec<(String, f32)> = components
        .iter()
        .filter_map(|c| c.temperature().filter(|t| t.is_finite() && *t > 0.0).map(|t| (c.label().to_string(), t)))
        .collect();
    temps.sort_by(|a, b| b.1.total_cmp(&a.1));
    temps
}

/// The fans, read from the SMC. Empty on a Mac without a fan (MacBook Air) or when the SMC
/// does not answer.
pub fn fans() -> Vec<Fan> {
    let Some(smc) = Smc::open() else { return Vec::new() };
    // FNum is a single byte; more than ten fans would be a misread, and the keys only go to F9.
    let count = smc.read("FNum").filter(|n| (0.0..=10.0).contains(n)).unwrap_or(0.0) as usize;
    (0..count.min(10))
        .filter_map(|n| {
            let rpm = smc.read(&format!("F{n}Ac")).filter(|r| *r >= 0.0)?;
            let min = smc.read(&format!("F{n}Mn")).filter(|r| *r >= 0.0);
            let max = smc.read(&format!("F{n}Mx")).filter(|r| *r > 0.0);
            Some(Fan { rpm, min, max })
        })
        .collect()
}

/// The battery, `None` on a desktop Mac.
pub fn battery() -> Option<Battery> {
    parse_battery(&super::collect::output("/usr/sbin/ioreg", &["-rn", "AppleSmartBattery"])?)
}

/// The processor speed limit the system applies, `None` when `pmset` says nothing.
pub fn thermal() -> Option<Thermal> {
    parse_thermal(&super::collect::output("/usr/bin/pmset", &["-g", "therm"])?)
}

/// Reads `ioreg -rn AppleSmartBattery`. `None` when no battery is installed.
pub fn parse_battery(ioreg: &str) -> Option<Battery> {
    if value(ioreg, "BatteryInstalled") == Some("No") {
        return None;
    }
    let number = |key: &str| value(ioreg, key).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let yes = |key: &str| value(ioreg, key) == Some("Yes");
    let design = number("DesignCapacity").filter(|d| *d > 0.0)?;
    // Apple Silicon gives the charge in percent (MaxCapacity = 100), Intel in mAh: the ratio
    // is the charge in both cases.
    let max = number("MaxCapacity").filter(|m| *m > 0.0)?;
    let charge = (number("CurrentCapacity")? / max).clamp(0.0, 1.0) as f32;
    // MaxCapacity is a percentage on Apple Silicon, so the raw value carries the wear there.
    let full = number("AppleRawMaxCapacity").or((max > 100.0).then_some(max));
    let health = full.filter(|f| *f > 0.0).map(|f| (f / design) as f32);
    // Centi-degrees Celsius.
    let temperature = number("Temperature").filter(|t| *t > 0.0).map(|t| (t / 100.0) as f32);
    // 65535 means « still calculating »; 0 shows up when there is nothing to estimate.
    let minutes_left = number("TimeRemaining").filter(|m| *m > 0.0 && *m < 65_535.0).map(|m| m as u32);
    Some(Battery {
        charge,
        cycles: number("CycleCount").filter(|c| *c >= 0.0).map_or(0, |c| c as u32),
        health,
        charging: yes("IsCharging"),
        external: yes("ExternalConnected"),
        temperature,
        minutes_left,
    })
}

/// The value of a top-level `"Key" = value` line. Matching at the start of the trimmed line
/// skips the same keys inside nested dictionaries, which `ioreg` prints on one line.
fn value<'a>(ioreg: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("\"{key}\" = ");
    ioreg.lines().find_map(|l| l.trim().strip_prefix(prefix.as_str())).map(str::trim)
}

/// Reads `pmset -g therm`.
pub fn parse_thermal(pmset: &str) -> Option<Thermal> {
    let limit = pmset.lines().find_map(|l| {
        let (key, value) = l.trim().split_once('=')?;
        (key.trim() == "CPU_Speed_Limit").then(|| value.trim().parse::<u32>().ok())?
    });
    match limit {
        Some(l) if l < 100 => Some(Thermal::Throttled(l as u8)),
        Some(_) => Some(Thermal::Nominal),
        None if pmset.contains("No thermal warning level has been recorded") => Some(Thermal::Nominal),
        None => None,
    }
}

/// An SMC value as a number, from its four-character type and its bytes.
fn decode(data_type: [u8; 4], bytes: &[u8]) -> Option<f32> {
    let value = match &data_type {
        // Apple Silicon: a native little-endian float.
        b"flt " => f32::from_le_bytes(bytes.get(..4)?.try_into().ok()?),
        // Intel: unsigned fixed point, 14 integer bits and 2 fraction bits, big-endian.
        b"fpe2" => f32::from(u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?)) / 4.0,
        // Signed fixed point, 7 integer bits and 8 fraction bits (Intel temperatures).
        b"sp78" => f32::from(i16::from_be_bytes(bytes.get(..2)?.try_into().ok()?)) / 256.0,
        b"ui8 " => f32::from(*bytes.first()?),
        b"ui16" => f32::from(u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?)),
        b"ui32" => u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

/// The SMC's request and answer, `SMCKeyData_t` in Apple's AppleSMC user client.
#[repr(C)]
#[derive(Default)]
struct KeyData {
    key: u32,
    vers: Version,
    p_limit_data: PLimitData,
    key_info: KeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct Version {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Default)]
struct PLimitData {
    version: u16,
    length: u16,
    cpu: u32,
    gpu: u32,
    mem: u32,
}

#[repr(C)]
#[derive(Default)]
struct KeyInfo {
    data_size: u32,
    data_type: u32,
    attributes: u8,
}

// The kernel checks the structure size: a wrong layout would only be rejected at run time.
const _: () = assert!(std::mem::size_of::<KeyData>() == 80);

/// `kSMCHandleYPCEvent`, the user client's only method.
const SMC_CALL: u32 = 2;
const READ_BYTES: u8 = 5;
const READ_KEY_INFO: u8 = 9;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
    fn IOServiceOpen(service: u32, owning_task: u32, kind: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(connect: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOConnectCallStructMethod(
        connection: u32,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> i32;
}

/// An open connection to the AppleSMC driver, closed on drop.
struct Smc(u32);

impl Smc {
    fn open() -> Option<Self> {
        // SAFETY: the name is a NUL-terminated C string; the call returns null on failure.
        let matching = unsafe { IOServiceMatching(c"AppleSMC".as_ptr()) };
        if matching.is_null() {
            return None;
        }
        // SAFETY: `matching` is a valid dictionary, consumed by the call; 0 is kIOMainPortDefault.
        let service = unsafe { IOServiceGetMatchingService(0, matching) };
        if service == 0 {
            return None;
        }
        let mut connect = 0u32;
        // SAFETY: `service` is a live service object and `connect` a valid place for the handle;
        // the service is released once, whatever the outcome, as the connection holds its own.
        let opened = unsafe {
            #[allow(deprecated)]
            let task = libc::mach_task_self();
            let r = IOServiceOpen(service, task, 0, &mut connect);
            IOObjectRelease(service);
            r
        };
        (opened == 0 && connect != 0).then_some(Smc(connect))
    }

    fn call(&self, input: &KeyData) -> Option<KeyData> {
        let mut output = KeyData::default();
        let mut size = std::mem::size_of::<KeyData>();
        // SAFETY: both buffers are `KeyData`, 80 bytes, the size the driver expects and is told.
        let r = unsafe {
            IOConnectCallStructMethod(
                self.0,
                SMC_CALL,
                (input as *const KeyData).cast(),
                std::mem::size_of::<KeyData>(),
                (&mut output as *mut KeyData).cast(),
                &mut size,
            )
        };
        (r == 0 && output.result == 0).then_some(output)
    }

    /// The value of a four-character key, `None` if the Mac does not have it.
    fn read(&self, key: &str) -> Option<f32> {
        let key = u32::from_be_bytes(key.as_bytes().try_into().ok()?);
        let info = self.call(&KeyData { key, data8: READ_KEY_INFO, ..Default::default() })?.key_info;
        let request = KeyData { key, data8: READ_BYTES, key_info: KeyInfo { data_size: info.data_size, ..Default::default() }, ..Default::default() };
        let answer = self.call(&request)?;
        let len = (info.data_size as usize).min(answer.bytes.len());
        decode(info.data_type.to_be_bytes(), &answer.bytes[..len])
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: the connection was opened by `IOServiceOpen` and is closed only here.
        unsafe { IOServiceClose(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: &str = r#"+-o AppleSmartBattery  <class AppleSmartBattery, id 0x100000abc, registered, matched, active, busy 0 (1 ms), retain 8>
    {
      "BatteryData" = {"DesignCapacity"=9999,"CycleCount"=9,"Temperature"=9999}
      "TimeRemaining" = 345
      "AppleRawMaxCapacity" = 4380
      "CurrentCapacity" = 85
      "MaxCapacity" = 100
      "IsCharging" = Yes
      "ExternalConnected" = Yes
      "CycleCount" = 120
      "Temperature" = 3012
      "DesignCapacity" = 4563
      "BatteryInstalled" = Yes
      "Serial" = "TEST0000000000000"
    }
"#;

    const DESKTOP: &str = r#"+-o AppleSmartBattery  <class AppleSmartBattery, id 0x100000abc, registered, matched, active, busy 0 (2 ms), retain 8>
    {
      "Amperage" = 0
      "CurrentCapacity" = 0
      "MaxCapacity" = 0
      "CycleCount" = 0
      "TimeRemaining" = 0
      "DesignCapacity" = 0
      "BatteryInstalled" = No
    }
"#;

    #[test]
    fn a_laptop_battery_is_read_from_its_top_level_keys() {
        let b = parse_battery(LAPTOP).unwrap();
        assert!((b.charge - 0.85).abs() < 1e-6);
        assert_eq!(b.cycles, 120);
        assert!((b.health.unwrap() - 4380.0 / 4563.0).abs() < 1e-6);
        assert!(b.charging && b.external);
        assert!((b.temperature.unwrap() - 30.12).abs() < 1e-4);
        assert_eq!(b.minutes_left, Some(345));
    }

    #[test]
    fn a_battery_still_estimating_has_no_time_left() {
        let b = parse_battery(&LAPTOP.replace("\"TimeRemaining\" = 345", "\"TimeRemaining\" = 65535")).unwrap();
        assert_eq!(b.minutes_left, None);
    }

    #[test]
    fn an_intel_battery_in_mah_gives_the_same_ratios() {
        let intel = "\"CurrentCapacity\" = 3000\n\"MaxCapacity\" = 6000\n\"DesignCapacity\" = 6600\n\"CycleCount\" = 3\n";
        let b = parse_battery(intel).unwrap();
        assert!((b.charge - 0.5).abs() < 1e-6);
        assert!((b.health.unwrap() - 6000.0 / 6600.0).abs() < 1e-6);
        assert!(!b.charging && !b.external && b.temperature.is_none() && b.minutes_left.is_none());
    }

    #[test]
    fn a_desktop_has_no_battery() {
        assert_eq!(parse_battery(DESKTOP), None);
        assert_eq!(parse_battery(&DESKTOP.replace("\"BatteryInstalled\" = No\n", "")), None);
        assert_eq!(parse_battery(""), None);
    }

    #[test]
    fn the_speed_limit_tells_throttling() {
        let throttled = "Note: No thermal warning level has been recorded\n2026-11-09 10:00:00 +0100 CPU Power notify\n\tCPU_Scheduler_Limit \t= 100\n\tCPU_Available_CPUs \t= 14\n\tCPU_Speed_Limit \t= 80\n";
        assert_eq!(parse_thermal(throttled), Some(Thermal::Throttled(80)));
        assert_eq!(parse_thermal(&throttled.replace("= 80", "= 100")), Some(Thermal::Nominal));
        let quiet = "Note: No thermal warning level has been recorded\nNote: No performance warning level has been recorded\n";
        assert_eq!(parse_thermal(quiet), Some(Thermal::Nominal));
        assert_eq!(parse_thermal(""), None);
        assert_eq!(parse_thermal("something else entirely"), None);
    }

    #[test]
    fn smc_values_are_decoded_by_type() {
        assert_eq!(decode(*b"flt ", &1234.5f32.to_le_bytes()), Some(1234.5));
        assert_eq!(decode(*b"fpe2", &[0x1f, 0x40]), Some(2000.0)); // 0x1f40 = 8000, / 4
        assert_eq!(decode(*b"ui8 ", &[2]), Some(2.0));
        assert_eq!(decode(*b"ui16", &[0x01, 0x00]), Some(256.0));
        assert_eq!(decode(*b"sp78", &[0x2a, 0x80]), Some(42.5));
        assert_eq!(decode(*b"flt ", &[0, 0]), None);
        assert_eq!(decode(*b"flt ", &f32::NAN.to_le_bytes()), None);
        assert_eq!(decode(*b"ch8*", b"abcd"), None);
    }

    #[test]
    fn the_sample_fills_every_field() {
        let s = Sensors::sample();
        assert_eq!(s.temperatures.len(), 5);
        assert!(s.temperatures.windows(2).all(|w| w[0].1 >= w[1].1));
        assert_eq!(s.fans.len(), 1);
        assert!(s.fans[0].min.is_some() && s.fans[0].max.is_some());
        assert_eq!(s.thermal, Some(Thermal::Nominal));
        let b = s.battery.unwrap();
        assert!(b.health.is_some() && b.temperature.is_some() && b.minutes_left.is_some());
    }

    /// Passes on a Mac with or without fans, sensors and battery (CI runners have none).
    #[test]
    fn this_mac_s_sensors_are_consistent() {
        // A VM may have no sensor at all, which `sysinfo` reports as an error.
        let temps = sysinfo::Components::new_with_refreshed_list().map(|mut c| temperatures(&mut c)).unwrap_or_default();
        assert!(temps.iter().all(|(_, t)| *t > 0.0 && *t < 150.0));
        assert!(temps.windows(2).all(|w| w[0].1 >= w[1].1));
        for fan in fans() {
            assert!(fan.rpm >= 0.0 && fan.rpm < 20_000.0);
            if let (Some(min), Some(max)) = (fan.min, fan.max) {
                assert!(min <= max);
            }
        }
        if let Some(Thermal::Throttled(limit)) = thermal() {
            assert!(limit < 100);
        }
        if let Some(b) = battery() {
            assert!((0.0..=1.0).contains(&b.charge));
        }
    }
}
