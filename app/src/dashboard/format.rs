//! How the dashboard and the menu bar write their numbers: French decimals, decimal byte units.

use crate::overview::format_bytes;

/// A share as « 42 % ».
pub fn percent(f: f32) -> String {
    format!("{:.0} %", f * 100.0)
}

/// A number with a decimal comma, « 3,42 ».
pub fn decimal(v: f64, places: usize) -> String {
    format!("{v:.places$}").replace('.', ",")
}

/// A throughput as « 2,4 Mo/s ».
pub fn rate(bytes_per_s: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_s))
}

/// A length of time as « 3 j 4 h », « 4 h 12 min » or « 12 min ».
pub fn duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} j {h} h"),
    }
}

/// The raw sensor names of the SMC, in words.
pub fn sensor_name(raw: &str) -> String {
    let number = |prefix: &str| raw.strip_prefix(prefix).map(str::trim);
    if let Some(n) = number("PMU tdie") {
        format!("Puce, zone {n}")
    } else if let Some(n) = number("PMU tdev") {
        format!("Puce, composant {n}")
    } else if raw.starts_with("NAND") {
        "SSD".to_string()
    } else if raw.contains("Battery") || raw.contains("gas gauge") {
        "Batterie".to_string()
    } else if raw == "PMU tcal" {
        "Puce, calibrage".to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_activity_monitor() {
        assert_eq!(duration(59), "0 min");
        assert_eq!(duration(12 * 60), "12 min");
        assert_eq!(duration(4 * 3_600 + 12 * 60), "4 h 12 min");
        assert_eq!(duration(3 * 86_400 + 4 * 3_600 + 59), "3 j 4 h");
    }

    #[test]
    fn rates_and_shares_are_in_french() {
        assert_eq!(rate(2_400_000), "2,4 Mo/s");
        assert_eq!(percent(0.423), "42 %");
        assert_eq!(decimal(3.4167, 2), "3,42");
    }

    #[test]
    fn smc_sensor_names_are_put_in_words() {
        assert_eq!(sensor_name("PMU tdie8"), "Puce, zone 8");
        assert_eq!(sensor_name("PMU tdev3"), "Puce, composant 3");
        assert_eq!(sensor_name("NAND CH0 temp"), "SSD");
        assert_eq!(sensor_name("Autre"), "Autre");
    }
}
