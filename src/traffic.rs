// Live receive and send rates of the active interface.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const STEP: Duration = Duration::from_millis(500);
const KEEP: usize = 720;
// Faster than this means a counter reset, not traffic.
const MAX_BPS: f64 = 400e9;

#[derive(Default)]
pub struct Traffic {
    pub key: String,
    pub rx: VecDeque<f64>,
    pub tx: VecDeque<f64>,
    pub now_rx: f64,
    pub now_tx: f64,
    pub peak_rx: f64,
    pub peak_tx: f64,
    pub total_rx: u64,
    pub total_tx: u64,
    last: Option<(Instant, u64, u64)>,
}

impl Traffic {
    pub fn update(&mut self, key: &str, now: Instant, rx: u64, tx: u64) {
        if self.key != key {
            *self = Traffic { key: key.to_owned(), ..Default::default() };
        }
        if let Some((t, r0, t0)) = self.last {
            let dt = now.saturating_duration_since(t).as_secs_f64();
            if dt < 0.05 {
                return;
            }
            let (dr, dtx) = (delta(r0, rx), delta(t0, tx));
            let (br, bt) = (dr as f64 * 8.0 / dt, dtx as f64 * 8.0 / dt);
            if br <= MAX_BPS && bt <= MAX_BPS {
                self.push(br, bt, dr, dtx);
            }
        }
        self.last = Some((now, rx, tx));
    }

    fn push(&mut self, br: f64, bt: f64, dr: u64, dt: u64) {
        self.rx.push_back(br);
        self.tx.push_back(bt);
        if self.rx.len() > KEEP {
            self.rx.pop_front();
            self.tx.pop_front();
        }
        self.total_rx += dr;
        self.total_tx += dt;
        self.peak_rx = self.peak_rx.max(br);
        self.peak_tx = self.peak_tx.max(bt);
        // The headline number averages the last second.
        let avg = |q: &VecDeque<f64>| {
            let n = q.len().min(2);
            q.iter().rev().take(n).sum::<f64>() / n.max(1) as f64
        };
        self.now_rx = avg(&self.rx);
        self.now_tx = avg(&self.tx);
    }

    pub fn clear(&mut self) {
        *self = Traffic::default();
    }
}

// Handles 32-bit counters that wrap around.
fn delta(old: u64, new: u64) -> u64 {
    if new >= old {
        new - old
    } else if old <= u64::from(u32::MAX) {
        new + (1u64 << 32) - old
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_totals_and_reset() {
        let t0 = Instant::now();
        let mut t = Traffic::default();
        t.update("eth0", t0, 1000, 500);
        assert!(t.rx.is_empty());
        t.update("eth0", t0 + Duration::from_secs(1), 1_000_000 + 1000, 500 + 125_000);
        assert_eq!(t.rx.back().copied(), Some(8_000_000.0));
        assert_eq!(t.tx.back().copied(), Some(1_000_000.0));
        assert_eq!(t.total_rx, 1_000_000);
        t.update("wlan0", t0 + Duration::from_secs(2), 5, 5);
        assert!(t.rx.is_empty());
        assert_eq!(t.total_rx, 0);
    }

    #[test]
    fn wraps_32_bit_counters() {
        assert_eq!(delta(u64::from(u32::MAX) - 9, 10), 20);
        assert_eq!(delta(5_000_000_000, 10), 0);
    }
}
