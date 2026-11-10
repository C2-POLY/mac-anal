//! The last ten minutes of every measure, for the curves. Kept in memory only.

use std::collections::{BTreeMap, VecDeque};

use super::{Series, Snapshot, FAST};

/// How far back the curves go.
pub const SPAN: u64 = 600;
/// Points kept per series at most, whatever the clock does: ten minutes at one reading every 2 s.
pub const CAPACITY: usize = (SPAN / FAST.as_secs()) as usize;

/// One point: when, in seconds since 1970, and the value in the series' unit.
pub type Point = (u64, f64);

#[derive(Clone, Debug, Default)]
pub struct History {
    series: BTreeMap<Series, VecDeque<Point>>,
}

impl History {
    /// Adds the measures of a reading and forgets what is older than `SPAN`.
    pub fn record(&mut self, s: &Snapshot) {
        for (series, value) in values(s) {
            self.push(series, s.at, value);
        }
    }

    pub fn push(&mut self, series: Series, at: u64, value: f64) {
        let points = self.series.entry(series).or_default();
        points.push_back((at, value));
        while points.len() > CAPACITY || points.front().is_some_and(|p| p.0 + SPAN < at) {
            points.pop_front();
        }
    }

    pub fn points(&self, series: Series) -> Vec<Point> {
        self.series.get(&series).map(|p| p.iter().copied().collect()).unwrap_or_default()
    }

    /// Invented curves for the screenshot mode, ending at `end`.
    pub fn sample(end: u64) -> Self {
        let mut h = Self::default();
        let step = FAST.as_secs();
        for i in 0..CAPACITY as u64 {
            let at = end - (CAPACITY as u64 - 1 - i) * step;
            let t = i as f64 / 12.0;
            let wave = |base: f64, amp: f64, phase: f64| (base + amp * (t + phase).sin() + amp * 0.4 * (t * 2.7).cos()).max(0.0);
            h.push(Series::Cpu, at, wave(0.25, 0.12, 0.0).min(1.0));
            h.push(Series::Memory, at, wave(0.68, 0.03, 1.0).min(1.0));
            h.push(Series::Swap, at, 1.2e9 + 5e7 * (t / 4.0).sin());
            h.push(Series::Gpu, at, wave(0.3, 0.15, 2.0).min(1.0));
            h.push(Series::NetIn, at, wave(2.0e6, 1.5e6, 0.5));
            h.push(Series::NetOut, at, wave(3.0e5, 2.0e5, 1.5));
            h.push(Series::DiskRead, at, wave(1.5e7, 1.2e7, 0.8));
            h.push(Series::DiskWrite, at, wave(4.0e6, 3.0e6, 2.2));
            h.push(Series::Temperature, at, wave(52.0, 4.0, 0.3));
        }
        h
    }
}

/// The value of each series in a reading, when it was measured.
fn values(s: &Snapshot) -> Vec<(Series, f64)> {
    let mut v = Vec::new();
    if let Some(c) = s.overview.cpu {
        v.push((Series::Cpu, c as f64));
    }
    if let Some(m) = &s.memory {
        if m.total > 0 {
            v.push((Series::Memory, m.used() as f64 / m.total as f64));
        }
        v.push((Series::Swap, m.swap_used as f64));
    }
    if let Some(g) = s.gpus.iter().find_map(|g| g.usage) {
        v.push((Series::Gpu, g as f64));
    }
    if !s.interfaces.is_empty() {
        let (r, t) = s.network_rate();
        v.push((Series::NetIn, r as f64));
        v.push((Series::NetOut, t as f64));
    }
    if !s.volumes.is_empty() {
        let (r, w) = s.disk_rate();
        v.push((Series::DiskRead, r as f64));
        v.push((Series::DiskWrite, w as f64));
    }
    if let Some((_, t)) = s.sensors.temperatures.first() {
        v.push((Series::Temperature, *t as f64));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_keeps_ten_minutes_and_forgets_the_rest() {
        let mut h = History::default();
        for i in 0..1_000u64 {
            h.push(Series::Cpu, 1_000 + i * 2, i as f64);
        }
        let p = h.points(Series::Cpu);
        assert_eq!(p.len(), CAPACITY);
        let last = p.last().unwrap().0;
        assert!(p.iter().all(|(at, _)| at + SPAN >= last));
        assert_eq!(p.last().unwrap().1, 999.0);
    }

    #[test]
    fn a_gap_in_the_readings_drops_points_older_than_the_span() {
        let mut h = History::default();
        h.push(Series::Gpu, 100, 0.1);
        h.push(Series::Gpu, 102, 0.2);
        h.push(Series::Gpu, 100 + SPAN + 50, 0.3);
        assert_eq!(h.points(Series::Gpu), [(100 + SPAN + 50, 0.3)]);
    }

    #[test]
    fn a_reading_feeds_only_the_series_it_measured() {
        let mut h = History::default();
        h.record(&Snapshot::default());
        assert!(h.points(Series::Cpu).is_empty());
        h.record(&Snapshot::sample());
        for s in [Series::Cpu, Series::Memory, Series::Swap, Series::Gpu, Series::NetIn, Series::DiskWrite, Series::Temperature] {
            assert_eq!(h.points(s).len(), 1, "{s:?}");
        }
    }

    #[test]
    fn the_sample_spans_ten_minutes() {
        let h = History::sample(10_000);
        let p = h.points(Series::NetIn);
        assert_eq!(p.len(), CAPACITY);
        assert_eq!(p.last().unwrap().0, 10_000);
        assert!(10_000 - p[0].0 < SPAN);
    }
}
