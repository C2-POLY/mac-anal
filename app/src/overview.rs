//! The three historical measures and the byte formatting the whole app shares.

/// The three measures. Every measure is `None` until it has been read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overview {
    pub disk: Option<Usage>,
    pub memory: Option<Usage>,
    pub cpu: Option<f32>,
}

/// Bytes used out of a total.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}

const GB: u64 = 1_000_000_000;

impl Overview {
    /// Invented values for the screenshot mode: nothing is read on the Mac.
    pub fn sample() -> Self {
        Self {
            disk: Some(Usage { used: 312 * GB, total: 494 * GB }),
            memory: Some(Usage { used: 11 * GB, total: 16 * GB }),
            cpu: Some(0.23),
        }
    }
}

/// A size in bytes, in the decimal units macOS shows (1 Go = 10⁹ octets).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_bytes(0), "0 o");
        assert_eq!(format_bytes(999), "999 o");
        assert_eq!(format_bytes(1_500_000), "1,5 Mo");
        assert_eq!(format_bytes(16 * GB), "16 Go");
        assert_eq!(format_bytes(512 * GB), "512 Go");
        assert_eq!(format_bytes(2_000 * GB), "2,0 To");
    }

    #[test]
    fn the_sample_fills_every_card_and_the_default_none() {
        let sample = Overview::sample();
        assert!(sample.disk.is_some() && sample.memory.is_some() && sample.cpu.is_some());
        let empty = Overview::default();
        assert!(empty.disk.is_none() && empty.memory.is_none() && empty.cpu.is_none());
    }
}
