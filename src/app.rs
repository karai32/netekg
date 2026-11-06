// Application state, background workers and the event loop.

use crate::meta::{self, PublicInfo};
use crate::net::{self, Local, Wifi};
use crate::ping::{self, Role, Summary, Target};
use crate::speedtest::{Failure, Phase, Results, SpeedTest};
use crate::traffic::{self, Traffic};
use crate::ui;
use crate::util::lock;
use crate::verdict::{self, Diagnosis, Inputs};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::{cursor, execute, terminal};
use ratatui::layout::{Position, Rect};
use std::io::{self, Stdout};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PING_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Public {
    #[default]
    Loading,
    Ready(PublicInfo),
    Failed,
}

pub struct Shared {
    pub local: Mutex<Option<Local>>,
    pub wifi: Mutex<Option<Wifi>>,
    pub public: Mutex<Public>,
    pub traffic: Mutex<Traffic>,
    pub targets: Mutex<Vec<Arc<Target>>>,
    pub speed: Arc<SpeedTest>,
    stop: AtomicBool,
    public_gen: AtomicU64,
    tcp_checked: AtomicBool,
}

impl Shared {
    pub fn start() -> Arc<Shared> {
        let s = Arc::new(Shared {
            local: Mutex::new(None),
            wifi: Mutex::new(None),
            public: Mutex::new(Public::Loading),
            traffic: Mutex::new(Traffic::default()),
            targets: Mutex::new(Vec::new()),
            speed: SpeedTest::new(),
            stop: AtomicBool::new(false),
            public_gen: AtomicU64::new(0),
            tcp_checked: AtomicBool::new(false),
        });
        s.refresh_local(true);
        s.refresh_wifi();
        let w = s.clone();
        let _ = thread::Builder::new().name("watch".into()).spawn(move || w.watch());
        let w = s.clone();
        let _ = thread::Builder::new().name("traffic".into()).spawn(move || w.sample_traffic());
        s
    }

    #[cfg(test)]
    pub fn idle() -> Arc<Shared> {
        Arc::new(Shared {
            local: Mutex::new(None),
            wifi: Mutex::new(None),
            public: Mutex::new(Public::Loading),
            traffic: Mutex::new(Traffic::default()),
            targets: Mutex::new(Vec::new()),
            speed: SpeedTest::new(),
            stop: AtomicBool::new(true),
            public_gen: AtomicU64::new(0),
            tcp_checked: AtomicBool::new(false),
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in lock(&self.targets).iter() {
            t.stop();
        }
        self.speed.cancel();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn watch(self: Arc<Self>) {
        let mut tick: u64 = 0;
        while !self.stopped() {
            thread::sleep(Duration::from_secs(1));
            tick += 1;
            if tick.is_multiple_of(3) {
                self.refresh_local(false);
                self.check_icmp();
            }
            if tick.is_multiple_of(2) {
                self.refresh_wifi();
            }
            let failed = matches!(*lock(&self.public), Public::Failed);
            if tick.is_multiple_of(30) && failed && lock(&self.local).is_some() {
                self.fetch_public();
            }
        }
    }

    fn refresh_local(self: &Arc<Self>, first: bool) {
        let now = net::detect();
        let changed = {
            let mut cur = lock(&self.local);
            let key = |l: &Option<Local>| l.as_ref().map(|l| (l.key.clone(), l.ip, l.gateway));
            let changed = key(&cur) != key(&now);
            *cur = now.clone();
            changed
        };
        if changed || first {
            self.reset_targets(now.as_ref().and_then(|l| l.gateway));
            *lock(&self.wifi) = None;
            if now.is_some() {
                self.fetch_public();
            } else {
                *lock(&self.public) = Public::Failed;
            }
        }
    }

    // Switches pings to TCP once when ICMP is blocked.
    fn check_icmp(self: &Arc<Self>) {
        if ping::method() != ping::Method::Icmp || self.tcp_checked.load(Ordering::Relaxed) {
            return;
        }
        let silent = {
            let now = Instant::now();
            let ts = lock(&self.targets);
            let internet: Option<Summary> =
                ts.iter().find(|t| t.role == Role::Internet).map(|t| lock(&t.series).summary(now));
            internet.is_some_and(|s| s.done >= 8 && s.replies == 0)
        };
        if !silent || self.tcp_checked.swap(true, Ordering::Relaxed) {
            return;
        }
        let me = self.clone();
        let _ = thread::Builder::new().name("probe".into()).spawn(move || {
            if ping::tcp_works(ping::INTERNET) {
                ping::force_tcp();
                let gateway = lock(&me.local).as_ref().and_then(|l| l.gateway);
                me.reset_targets(gateway);
            } else {
                // Really offline; check again later.
                me.tcp_checked.store(false, Ordering::Relaxed);
            }
        });
    }

    fn refresh_wifi(&self) {
        let local = lock(&self.local).clone();
        let wifi = local.as_ref().and_then(net::wifi);
        *lock(&self.wifi) = wifi;
    }

    // Fresh targets on every network change.
    fn reset_targets(&self, gateway: Option<Ipv4Addr>) {
        let mut ts = lock(&self.targets);
        for t in ts.iter() {
            t.stop();
        }
        ts.clear();
        if let Some(g) = gateway {
            ts.push(Target::spawn(Role::Router, g, PING_EVERY));
        }
        ts.push(Target::spawn(Role::Internet, ping::INTERNET, PING_EVERY));
    }

    fn fetch_public(self: &Arc<Self>) {
        let generation = self.public_gen.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.public) = Public::Loading;
        let me = self.clone();
        let _ = thread::Builder::new().name("public".into()).spawn(move || {
            let result = meta::fetch();
            if me.public_gen.load(Ordering::SeqCst) == generation {
                *lock(&me.public) = match result {
                    Ok(p) => Public::Ready(p),
                    Err(_) => Public::Failed,
                };
            }
        });
    }

    fn sample_traffic(self: Arc<Self>) {
        while !self.stopped() {
            let local = lock(&self.local).clone();
            match local {
                Some(l) => {
                    if let Some((rx, tx)) = net::counters(&l) {
                        lock(&self.traffic).update(&l.key, Instant::now(), rx, tx);
                    }
                }
                None => lock(&self.traffic).clear(),
            }
            thread::sleep(traffic::STEP);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Speed,
    Cancel,
    Quit,
}

pub struct TargetView {
    pub role: Role,
    pub addr: Ipv4Addr,
    pub sum: Summary,
    pub history: Vec<Option<f64>>,
}

pub struct SpeedView {
    pub phase: Phase,
    pub live: f64,
    pub progress: f64,
    pub res: Results,
    pub failure: Option<Failure>,
    pub finished: Option<Instant>,
}

impl SpeedView {
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Latency | Phase::Download | Phase::Upload)
    }
}

// Snapshot of everything on screen, taken once per frame.
pub struct View {
    pub local: Option<Local>,
    pub wifi: Option<Wifi>,
    pub public: Public,
    pub rx: Vec<f64>,
    pub tx: Vec<f64>,
    pub now_rx: f64,
    pub now_tx: f64,
    pub peak_rx: f64,
    pub peak_tx: f64,
    pub total_rx: u64,
    pub total_tx: u64,
    pub targets: Vec<TargetView>,
    pub speed: SpeedView,
    pub diag: Diagnosis,
    pub tcp: bool,
}

pub struct App {
    pub shared: Arc<Shared>,
    pub hits: Vec<(Rect, Action)>,
    // Drives the spinners.
    pub epoch: Instant,
    // Redraw faster while something moves.
    pub animate: bool,
    quit: bool,
}

impl App {
    pub fn new(shared: Arc<Shared>) -> App {
        App { shared, hits: Vec::new(), epoch: Instant::now(), animate: false, quit: false }
    }

    pub fn view(&self) -> View {
        let local = lock(&self.shared.local).clone();
        let wifi = lock(&self.shared.wifi).clone();
        let public = lock(&self.shared.public).clone();
        let (rx, tx, now_rx, now_tx, peak_rx, peak_tx, total_rx, total_tx) = {
            let t = lock(&self.shared.traffic);
            (
                t.rx.iter().copied().collect(),
                t.tx.iter().copied().collect(),
                t.now_rx,
                t.now_tx,
                t.peak_rx,
                t.peak_tx,
                t.total_rx,
                t.total_tx,
            )
        };
        let now = Instant::now();
        let targets: Vec<TargetView> = lock(&self.shared.targets)
            .iter()
            .map(|t| {
                let s = lock(&t.series);
                TargetView { role: t.role, addr: t.addr, sum: s.summary(now), history: s.history(240) }
            })
            .collect();
        let speed = {
            let s = lock(&self.shared.speed.state);
            SpeedView {
                phase: s.phase,
                live: s.live,
                progress: s.progress(),
                res: s.res.clone(),
                failure: s.failure,
                finished: s.finished,
            }
        };
        let summary = |role: Role| targets.iter().find(|t| t.role == role).map(|t| t.sum);
        let diag = verdict::diagnose(&Inputs {
            has_network: local.is_some(),
            router: summary(Role::Router),
            internet: summary(Role::Internet),
            wifi_dbm: wifi.as_ref().and_then(|w| w.dbm),
        });
        View {
            local,
            wifi,
            public,
            rx,
            tx,
            now_rx,
            now_tx,
            peak_rx,
            peak_tx,
            total_rx,
            total_tx,
            targets,
            speed,
            diag,
            tcp: ping::method() == ping::Method::Tcp,
        }
    }

    fn run_action(&mut self, a: Action) {
        let speed = &self.shared.speed;
        match a {
            Action::Speed => {
                if !lock(&speed.state).running() {
                    speed.start();
                }
            }
            Action::Cancel => {
                if lock(&speed.state).running() {
                    speed.cancel();
                }
            }
            Action::Quit => self.quit = true,
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c' | 'C')) {
            self.quit = true;
            return;
        }
        match k.code {
            KeyCode::Char('q' | 'Q') => self.run_action(Action::Quit),
            KeyCode::Char('s' | 'S') => self.run_action(Action::Speed),
            KeyCode::Esc => self.run_action(Action::Cancel),
            _ => {}
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        if m.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let p = Position::new(m.column, m.row);
        if let Some(a) = self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, a)| *a) {
            self.run_action(a);
        }
    }

    fn event_loop(&mut self, term: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
        while !self.quit {
            term.draw(|f| ui::draw(f, self))?;
            let wait = if self.animate { 100 } else { 250 };
            if event::poll(Duration::from_millis(wait))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
                        Event::Mouse(m) => self.mouse(m),
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), event::DisableMouseCapture, terminal::LeaveAlternateScreen, cursor::Show);
}

pub fn run() -> io::Result<()> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only the main thread restores the terminal on panic.
        if thread::current().name() == Some("main") {
            restore();
            hook(info);
        }
    }));
    let shared = Shared::start();
    let mut app = App::new(shared.clone());
    terminal::enable_raw_mode()?;
    let setup = (|| -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
        execute!(io::stdout(), terminal::EnterAlternateScreen, event::EnableMouseCapture, cursor::Hide)?;
        let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        term.clear()?;
        Ok(term)
    })();
    let result = match setup {
        Ok(mut term) => app.event_loop(&mut term),
        Err(e) => Err(e),
    };
    restore();
    shared.stop();
    result
}
