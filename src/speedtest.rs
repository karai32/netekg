// Speed test against speed.cloudflare.com: ping, download, upload and ping under load.

use crate::http::{CONNECT_TIMEOUT, Conn, HOST};
use crate::ping::{self, Outcome, Role, Target};
use crate::util::{jitter, lock, median};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const LATENCY: Duration = Duration::from_secs(2);
pub const PHASE: Duration = Duration::from_secs(10);
pub const TICK: Duration = Duration::from_millis(100);
const DOWN_STREAMS: usize = 6;
const UP_STREAMS: usize = 6;
// Cloudflare rate-limits requests of 10 MB and more.
const DOWN_BYTES: u64 = 4_000_000;
const UP_BYTES: u64 = 10_000_000;
// The first 30% of each phase is ramp-up and is not counted.
const WARMUP: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Latency,
    Download,
    Upload,
    Done,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Server,
    NoData,
    Limited,
}

impl Failure {
    pub fn describe(self) -> &'static str {
        match self {
            Failure::Server => "can't reach the test server (speed.cloudflare.com)",
            Failure::NoData => "the test server sends no data",
            Failure::Limited => {
                "Cloudflare limits how often the test runs from one address, try again later (usually within an hour)"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    APlus,
    A,
    B,
    C,
    D,
    F,
}

impl Grade {
    // Thresholds follow the common bufferbloat grading.
    pub fn from_increase(ms: f64) -> Grade {
        match ms {
            m if m < 5.0 => Grade::APlus,
            m if m < 30.0 => Grade::A,
            m if m < 60.0 => Grade::B,
            m if m < 200.0 => Grade::C,
            m if m < 400.0 => Grade::D,
            _ => Grade::F,
        }
    }

    pub fn label(self) -> &'static str {
        ["A+", "A", "B", "C", "D", "F"][self as usize]
    }

    pub fn describe(self) -> &'static str {
        [
            "none at all",
            "barely noticeable",
            "fine for most things",
            "noticeable when busy",
            "strong when busy",
            "the line chokes",
        ][self as usize]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub idle_ping: Option<f64>,
    pub idle_jitter: Option<f64>,
    pub down_ping: Option<f64>,
    pub up_ping: Option<f64>,
    pub down: Option<f64>,
    pub up: Option<f64>,
    pub bytes: u64,
    pub server: Option<String>,
}

impl Results {
    pub fn lag_increase(&self) -> Option<f64> {
        let idle = self.idle_ping?;
        let loaded = match (self.down_ping, self.up_ping) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return None,
        };
        Some((loaded - idle).max(0.0))
    }

    pub fn grade(&self) -> Option<Grade> {
        self.lag_increase().map(Grade::from_increase)
    }
}

pub struct State {
    pub phase: Phase,
    pub phase_started: Instant,
    pub live: f64,
    pub res: Results,
    pub failure: Option<Failure>,
    pub finished: Option<Instant>,
}

impl State {
    fn new() -> State {
        State {
            phase: Phase::Idle,
            phase_started: Instant::now(),
            live: 0.0,
            res: Results::default(),
            failure: None,
            finished: None,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Latency | Phase::Download | Phase::Upload)
    }

    // Share of the whole test that is done, 0..1.
    pub fn progress(&self) -> f64 {
        let total = LATENCY + PHASE * 2;
        let el = self.phase_started.elapsed();
        let done = match self.phase {
            Phase::Latency => el.min(LATENCY),
            Phase::Download => LATENCY + el.min(PHASE),
            Phase::Upload => LATENCY + PHASE + el.min(PHASE),
            Phase::Done => total,
            _ => Duration::ZERO,
        };
        done.as_secs_f64() / total.as_secs_f64()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Down,
    Up,
}

enum Step {
    Again,
    Reconnect,
    Stopped,
    Limited,
}

pub struct SpeedTest {
    pub state: Mutex<State>,
    cancel: AtomicBool,
    busy: AtomicBool,
}

impl SpeedTest {
    pub fn new() -> Arc<SpeedTest> {
        Arc::new(SpeedTest {
            state: Mutex::new(State::new()),
            cancel: AtomicBool::new(false),
            busy: AtomicBool::new(false),
        })
    }

    pub fn start(self: &Arc<Self>) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        {
            let mut s = lock(&self.state);
            *s = State::new();
            s.phase = Phase::Latency;
        }
        let me = self.clone();
        let spawned = thread::Builder::new().name("speedtest".into()).spawn(move || {
            me.run();
            me.busy.store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.end(Phase::Failed, Some(Failure::Server));
            self.busy.store(false, Ordering::SeqCst);
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn end(&self, phase: Phase, failure: Option<Failure>) {
        let mut s = lock(&self.state);
        s.phase = phase;
        s.failure = failure;
        s.live = 0.0;
        s.finished = Some(Instant::now());
    }

    fn run(&self) {
        let probe = thread::spawn(server_probe);
        let pinger = Target::spawn(Role::Internet, ping::INTERNET, Duration::from_millis(100));
        let start = Instant::now();
        while start.elapsed() < LATENCY && !self.cancelled() {
            thread::sleep(TICK);
        }
        pinger.stop();
        let server = probe.join().ok().flatten();
        if self.cancelled() {
            return self.end(Phase::Cancelled, None);
        }
        let Some(server) = server else {
            return self.end(Phase::Failed, Some(Failure::Server));
        };
        let idle = replies(&pinger, start);
        {
            let mut s = lock(&self.state);
            s.res.server = (!server.is_empty()).then_some(server);
            s.res.idle_ping = median(&idle);
            s.res.idle_jitter = jitter(&idle);
        }
        for dir in [Dir::Down, Dir::Up] {
            if let Err((phase, failure)) = self.transfer(dir) {
                return self.end(phase, failure);
            }
        }
        self.end(Phase::Done, None);
    }

    fn transfer(&self, dir: Dir) -> Result<(), (Phase, Option<Failure>)> {
        let counter = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let limited = Arc::new(AtomicBool::new(false));
        let streams = if dir == Dir::Down { DOWN_STREAMS } else { UP_STREAMS };
        for _ in 0..streams {
            let (c, s, l) = (counter.clone(), stop.clone(), limited.clone());
            let _ = thread::Builder::new().name("transfer".into()).spawn(move || worker(dir, &c, &s, &l));
        }
        let pinger = Target::spawn(Role::Internet, ping::INTERNET, Duration::from_millis(250));
        let start = Instant::now();
        let base = {
            let mut s = lock(&self.state);
            s.phase = if dir == Dir::Down { Phase::Download } else { Phase::Upload };
            s.phase_started = start;
            s.live = 0.0;
            s.res.bytes
        };
        let mut marks: Vec<(f64, u64)> = vec![(0.0, 0)];
        let outcome = loop {
            thread::sleep(TICK);
            let t = start.elapsed().as_secs_f64();
            let b = counter.load(Ordering::Relaxed);
            marks.push((t, b));
            let live = rate_since(&marks, t - 0.5);
            {
                let mut s = lock(&self.state);
                s.live = live;
                s.res.bytes = base + b;
            }
            if self.cancelled() {
                break Err((Phase::Cancelled, None));
            }
            if t >= PHASE.as_secs_f64() {
                break Ok(());
            }
            if t >= 5.0 && b == 0 {
                let why = if limited.load(Ordering::Relaxed) { Failure::Limited } else { Failure::NoData };
                break Err((Phase::Failed, Some(why)));
            }
        };
        stop.store(true, Ordering::Relaxed);
        pinger.stop();
        outcome?;
        let mbps = steady_rate(&marks, PHASE.as_secs_f64() * WARMUP);
        let loaded = median(&replies(&pinger, start + PHASE.mul_f64(WARMUP)));
        let mut s = lock(&self.state);
        if dir == Dir::Down {
            s.res.down = Some(mbps);
            s.res.down_ping = loaded;
        } else {
            s.res.up = Some(mbps);
            s.res.up_ping = loaded;
        }
        Ok(())
    }
}

fn replies(t: &Target, since: Instant) -> Vec<f64> {
    lock(&t.series)
        .samples
        .iter()
        .filter(|s| s.sent >= since)
        .filter_map(|s| match s.outcome {
            Outcome::Reply(v) => Some(v),
            _ => None,
        })
        .collect()
}

// Mbit/s between the last mark at or before `t0` and the newest mark.
fn rate_since(marks: &[(f64, u64)], t0: f64) -> f64 {
    let Some(&(t1, b1)) = marks.last() else { return 0.0 };
    let (ta, ba) = marks.iter().rev().find(|(t, _)| *t <= t0).copied().unwrap_or(marks[0]);
    if t1 - ta <= 0.0 {
        return 0.0;
    }
    (b1 - ba) as f64 * 8.0 / (t1 - ta) / 1e6
}

fn steady_rate(marks: &[(f64, u64)], warm: f64) -> f64 {
    let Some(&(t1, b1)) = marks.last() else { return 0.0 };
    let (ta, ba) = marks.iter().find(|(t, _)| *t >= warm).copied().unwrap_or(marks[0]);
    if t1 - ta <= 0.0 {
        return rate_since(marks, 0.0);
    }
    (b1 - ba) as f64 * 8.0 / (t1 - ta) / 1e6
}

fn server_probe() -> Option<String> {
    let mut c = Conn::open(HOST, CONNECT_TIMEOUT).ok()?;
    c.send_head("GET", "/__down?bytes=0", &[], None).ok()?;
    let h = c.read_head().ok()?;
    if h.status != 200 {
        return None;
    }
    let _ = c.read_body(&h, &mut |_| true);
    let colo = h.get("colo").map(str::to_owned).or_else(|| h.get("cf-ray")?.rsplit('-').next().map(str::to_owned));
    Some(colo.unwrap_or_default())
}

fn worker(dir: Dir, counter: &AtomicU64, stop: &AtomicBool, limited: &AtomicBool) {
    let mut conn: Option<Conn> = None;
    while !stop.load(Ordering::Relaxed) {
        if conn.is_none() {
            match Conn::open(HOST, CONNECT_TIMEOUT) {
                Ok(c) => conn = Some(c),
                Err(_) => {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }
        let Some(c) = conn.as_mut() else { continue };
        let step = match dir {
            Dir::Down => download(c, counter, stop),
            Dir::Up => upload(c, counter, stop),
        };
        match step {
            Ok(Step::Again) => {}
            Ok(Step::Reconnect) => conn = None,
            Ok(Step::Stopped) => break,
            Ok(Step::Limited) => {
                limited.store(true, Ordering::Relaxed);
                conn = None;
                thread::sleep(Duration::from_secs(1));
            }
            Err(_) => {
                conn = None;
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

// Keeps a second request queued so the line never idles.
fn download(c: &mut Conn, counter: &AtomicU64, stop: &AtomicBool) -> io::Result<Step> {
    let path = format!("/__down?bytes={DOWN_BYTES}");
    c.send_head("GET", &path, &[], None)?;
    c.send_head("GET", &path, &[], None)?;
    loop {
        let h = c.read_head()?;
        if h.status == 429 {
            return Ok(Step::Limited);
        }
        if h.status != 200 {
            return Err(io::Error::other(format!("HTTP {}", h.status)));
        }
        let done = c.read_body(&h, &mut |b| {
            counter.fetch_add(b.len() as u64, Ordering::Relaxed);
            !stop.load(Ordering::Relaxed)
        })?;
        if !done {
            return Ok(Step::Stopped);
        }
        if !h.keep_alive() {
            return Ok(Step::Reconnect);
        }
        c.send_head("GET", &path, &[], None)?;
    }
}

fn upload(c: &mut Conn, counter: &AtomicU64, stop: &AtomicBool) -> io::Result<Step> {
    c.send_head("POST", "/__up", &[("Content-Type", "application/octet-stream")], Some(UP_BYTES))?;
    let data = payload();
    let mut left = UP_BYTES;
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return Ok(Step::Stopped);
        }
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(data.len());
        c.write_body(&data[..n])?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
        left -= n as u64;
    }
    c.flush()?;
    let h = c.read_head()?;
    c.read_body(&h, &mut |_| true)?;
    match h.status {
        200 => Ok(if h.keep_alive() { Step::Again } else { Step::Reconnect }),
        429 => Ok(Step::Limited),
        code => Err(io::Error::other(format!("HTTP {code}"))),
    }
}

// Incompressible upload data.
fn payload() -> &'static [u8] {
    static P: OnceLock<Vec<u8>> = OnceLock::new();
    P.get_or_init(|| {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        (0..64 * 1024)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades() {
        assert_eq!(Grade::from_increase(2.0), Grade::APlus);
        assert_eq!(Grade::from_increase(29.0), Grade::A);
        assert_eq!(Grade::from_increase(45.0), Grade::B);
        assert_eq!(Grade::from_increase(150.0), Grade::C);
        assert_eq!(Grade::from_increase(300.0), Grade::D);
        assert_eq!(Grade::from_increase(900.0), Grade::F);
        assert_eq!(Grade::B.label(), "B");
    }

    #[test]
    fn lag_uses_worst_loaded_ping() {
        let r = Results { idle_ping: Some(20.0), down_ping: Some(45.0), up_ping: Some(80.0), ..Default::default() };
        assert_eq!(r.lag_increase(), Some(60.0));
        assert_eq!(r.grade(), Some(Grade::C));
        assert_eq!(Results::default().grade(), None);
    }

    #[test]
    fn rates_skip_warmup() {
        // 10 MB per second after a slow first second.
        let marks: Vec<(f64, u64)> =
            vec![(0.0, 0), (1.0, 1_000_000), (2.0, 11_000_000), (3.0, 21_000_000), (4.0, 31_000_000)];
        assert!((steady_rate(&marks, 1.5) - 80.0).abs() < 1e-9);
        assert!((rate_since(&marks, 3.0) - 80.0).abs() < 1e-9);
        assert_eq!(steady_rate(&[], 1.0), 0.0);
    }

    #[test]
    fn payload_is_not_repetitive() {
        let p = payload();
        assert_eq!(p.len(), 64 * 1024);
        let zeros = p.iter().filter(|&&b| b == 0).count();
        assert!(zeros < 1024);
    }
}
