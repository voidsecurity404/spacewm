//! Performance numbers for `spacectl stats`: frame times over the last seconds, how much
//! work frames did, and a GPU memory estimate.
//!
//! Frame time here is the CPU time from "start rendering" to "frame handed to the
//! backend" (scene, panel textures, compositing) — the part we control. Percentiles say
//! more than an average: p95 = 95% of frames were at least this fast, so p95/p99 show
//! the stutters you'd feel.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Frames kept for percentiles (10 s at 60 Hz).
const KEEP_FRAMES: usize = 600;
/// Rates (fps, updates/s) are measured over this window.
const RATE_WINDOW: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Perf {
    frames: VecDeque<(Instant, Duration)>,
    panel_updates: VecDeque<(Instant, u32)>,
    chunk_uploads: VecDeque<(Instant, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    pub fps: f64,
    pub frame_ms_p50: f64,
    pub frame_ms_p95: f64,
    pub frame_ms_p99: f64,
    pub frame_ms_max: f64,
    pub panel_updates_per_s: f64,
    pub chunk_uploads_per_s: f64,
}

fn push(q: &mut VecDeque<(Instant, u32)>, now: Instant, n: u32) {
    if n > 0 {
        q.push_back((now, n));
    }
    while q.front().is_some_and(|(t, _)| now.duration_since(*t) > RATE_WINDOW) {
        q.pop_front();
    }
}

/// The value below which `p` (0..1) of the sorted values lie (nearest rank).
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

impl Perf {
    pub fn record_frame(
        &mut self,
        start: Instant,
        cpu: Duration,
        panel_updates: u32,
        chunk_uploads: u32,
    ) {
        let now = start + cpu;
        self.frames.push_back((now, cpu));
        while self.frames.len() > KEEP_FRAMES {
            self.frames.pop_front();
        }
        push(&mut self.panel_updates, now, panel_updates);
        push(&mut self.chunk_uploads, now, chunk_uploads);
    }

    pub fn snapshot(&self, now: Instant) -> Snapshot {
        let recent = |q: &VecDeque<(Instant, u32)>| -> f64 {
            let n: u32 = q
                .iter()
                .filter(|(t, _)| now.duration_since(*t) <= RATE_WINDOW)
                .map(|(_, n)| n)
                .sum();
            n as f64 / RATE_WINDOW.as_secs_f64()
        };
        let fps = self.frames.iter().filter(|(t, _)| now.duration_since(*t) <= RATE_WINDOW).count()
            as f64
            / RATE_WINDOW.as_secs_f64();
        let mut ms: Vec<f64> = self.frames.iter().map(|(_, d)| d.as_secs_f64() * 1000.0).collect();
        ms.sort_by(f64::total_cmp);
        Snapshot {
            fps,
            frame_ms_p50: percentile(&ms, 0.50),
            frame_ms_p95: percentile(&ms, 0.95),
            frame_ms_p99: percentile(&ms, 0.99),
            frame_ms_max: ms.last().copied().unwrap_or(0.0),
            panel_updates_per_s: recent(&self.panel_updates),
            chunk_uploads_per_s: recent(&self.chunk_uploads),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), 50.0);
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(percentile(&v, 0.99), 99.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.99), 7.0);
    }

    #[test]
    fn rates_cover_the_window_only() {
        let mut p = Perf::default();
        let t0 = Instant::now();
        for i in 0..120 {
            // 60 fps for 2 s, one panel update per frame.
            p.record_frame(t0 + Duration::from_millis(i * 16), Duration::from_millis(2), 1, 0);
        }
        let s = p.snapshot(t0 + Duration::from_millis(119 * 16 + 2));
        assert!((s.fps - 60.0).abs() < 2.0, "{s:?}");
        assert!((s.panel_updates_per_s - 60.0).abs() < 2.0, "{s:?}");
        assert_eq!(s.frame_ms_p95, 2.0);
        // Much later: nothing recent.
        let later = p.snapshot(t0 + Duration::from_secs(30));
        assert_eq!(later.fps, 0.0);
    }
}
