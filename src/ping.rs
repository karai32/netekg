// Ping: ICMP echo without root where possible, TCP handshakes otherwise.

use crate::util::{jitter, lock, median};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(2);
pub const WINDOW: Duration = Duration::from_secs(60);
const KEEP: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Icmp,
    Tcp,
}

// 0 until the first probe, then 1 for ICMP or 2 for TCP.
static METHOD: AtomicU8 = AtomicU8::new(0);

pub fn method() -> Method {
    match METHOD.load(Ordering::Relaxed) {
        1 => Method::Icmp,
        2 => Method::Tcp,
        _ => {
            let m = if sys::available() { 1 } else { 2 };
            let _ = METHOD.compare_exchange(0, m, Ordering::Relaxed, Ordering::Relaxed);
            method()
        }
    }
}

// For networks that drop ICMP.
pub fn force_tcp() {
    METHOD.store(2, Ordering::Relaxed);
}

// True when a TCP handshake with the host succeeds.
pub fn tcp_works(host: Ipv4Addr) -> bool {
    matches!(tcp(host, 443, TIMEOUT), Outcome::Reply(_))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Pending,
    Reply(f64),
    Lost,
}

pub fn ping_once(addr: Ipv4Addr, ident: u16, seq: u16, tcp_port: u16) -> Outcome {
    match method() {
        Method::Icmp => sys::echo(addr, ident, seq, TIMEOUT),
        Method::Tcp => tcp(addr, tcp_port, TIMEOUT),
    }
}

fn tcp(addr: Ipv4Addr, port: u16, timeout: Duration) -> Outcome {
    let start = Instant::now();
    match TcpStream::connect_timeout(&SocketAddr::from((addr, port)), timeout) {
        Ok(_) => Outcome::Reply(ms(start)),
        // A reset also proves the host answered.
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Outcome::Reply(ms(start)),
        Err(_) => Outcome::Lost,
    }
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub seq: u32,
    pub sent: Instant,
    pub outcome: Outcome,
}

#[derive(Default)]
pub struct Series {
    pub samples: VecDeque<Sample>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub last: Option<f64>,
    pub median: Option<f64>,
    pub jitter: Option<f64>,
    // Percent of finished pings in the last minute that got no reply.
    pub loss: Option<f64>,
    pub done: usize,
    pub replies: usize,
    pub lost_streak: usize,
}

impl Summary {
    pub fn loss_or_zero(&self) -> f64 {
        self.loss.unwrap_or(0.0)
    }
}

impl Series {
    pub fn begin(&mut self, seq: u32, sent: Instant) {
        self.samples.push_back(Sample { seq, sent, outcome: Outcome::Pending });
        while self.samples.len() > KEEP {
            self.samples.pop_front();
        }
    }

    pub fn finish(&mut self, seq: u32, outcome: Outcome) {
        if let Some(s) = self.samples.iter_mut().rev().find(|s| s.seq == seq) {
            s.outcome = outcome;
        }
    }

    pub fn summary(&self, now: Instant) -> Summary {
        let mut sum = Summary::default();
        let mut recent = Vec::new();
        let mut streak_open = true;
        for s in self.samples.iter().rev() {
            if now.saturating_duration_since(s.sent) > WINDOW {
                break;
            }
            match s.outcome {
                Outcome::Pending => continue,
                Outcome::Reply(v) => {
                    sum.replies += 1;
                    streak_open = false;
                    sum.last.get_or_insert(v);
                    if recent.len() < 30 {
                        recent.push(v);
                    }
                }
                Outcome::Lost => {
                    if streak_open {
                        sum.lost_streak += 1;
                    }
                }
            }
            sum.done += 1;
        }
        recent.reverse();
        let tail = &recent[recent.len().saturating_sub(10)..];
        sum.median = median(tail);
        sum.jitter = jitter(&recent);
        if sum.done > 0 {
            sum.loss = Some((sum.done - sum.replies) as f64 * 100.0 / sum.done as f64);
        }
        sum
    }

    // Last `n` round trips, oldest first; None is a lost ping.
    pub fn history(&self, n: usize) -> Vec<Option<f64>> {
        let mut out: Vec<Option<f64>> = self
            .samples
            .iter()
            .rev()
            .filter(|s| s.outcome != Outcome::Pending)
            .take(n)
            .map(|s| match s.outcome {
                Outcome::Reply(v) => Some(v),
                _ => None,
            })
            .collect();
        out.reverse();
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Router,
    Internet,
}

impl Role {
    pub fn tcp_port(self) -> u16 {
        if self == Role::Router { 80 } else { 443 }
    }
}

// Cloudflare's resolver stands for the internet.
pub const INTERNET: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);

pub struct Target {
    pub role: Role,
    pub addr: Ipv4Addr,
    pub series: Mutex<Series>,
    stop: AtomicBool,
}

impl Target {
    pub fn spawn(role: Role, addr: Ipv4Addr, interval: Duration) -> Arc<Target> {
        let t = Arc::new(Target { role, addr, series: Mutex::new(Series::default()), stop: AtomicBool::new(false) });
        let worker = t.clone();
        let _ = thread::Builder::new().name("ping".into()).spawn(move || worker.run(interval));
        t
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn run(self: Arc<Self>, interval: Duration) {
        static IDENT: AtomicU16 = AtomicU16::new(0);
        let ident = (std::process::id() as u16).wrapping_add(IDENT.fetch_add(0x101, Ordering::Relaxed));
        let mut seq: u32 = 0;
        let mut next = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if next > now {
                thread::sleep((next - now).min(Duration::from_millis(200)));
                continue;
            }
            seq += 1;
            lock(&self.series).begin(seq, Instant::now());
            let me = self.clone();
            let s = seq;
            let spawned = thread::Builder::new().name("echo".into()).stack_size(256 * 1024).spawn(move || {
                let outcome = ping_once(me.addr, ident, s as u16, me.role.tcp_port());
                lock(&me.series).finish(s, outcome);
            });
            if spawned.is_err() {
                lock(&self.series).finish(seq, Outcome::Lost);
            }
            next += interval;
            if next < Instant::now() {
                next = Instant::now() + interval;
            }
        }
    }
}

#[cfg(unix)]
mod sys {
    use super::{Outcome, ms};
    use socket2::{Domain, Protocol, Socket, Type};
    use std::io::ErrorKind;
    use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
    use std::time::{Duration, Instant};

    // Unprivileged ICMP socket first, then a raw one.
    fn open() -> std::io::Result<(UdpSocket, bool)> {
        match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)) {
            Ok(s) => Ok((s.into(), false)),
            Err(_) => Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).map(|s| (s.into(), true)),
        }
    }

    pub fn available() -> bool {
        open().is_ok()
    }

    pub fn echo(addr: Ipv4Addr, ident: u16, seq: u16, timeout: Duration) -> Outcome {
        let Ok((sock, raw)) = open() else { return Outcome::Lost };
        // Linux sets the identifier on unprivileged sockets itself.
        let check_id = raw || !cfg!(any(target_os = "linux", target_os = "android"));
        let pkt = packet(ident, seq);
        let start = Instant::now();
        if sock.send_to(&pkt, SocketAddrV4::new(addr, 0)).is_err() {
            return Outcome::Lost;
        }
        let mut buf = [0u8; 1500];
        loop {
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() || sock.set_read_timeout(Some(left)).is_err() {
                return Outcome::Lost;
            }
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if from.ip() != IpAddr::V4(addr) {
                        continue;
                    }
                    if let Some((kind, id, sq)) = parse_reply(&buf[..n])
                        && kind == 0
                        && sq == seq
                        && (!check_id || id == ident)
                    {
                        return Outcome::Reply(ms(start));
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Outcome::Lost,
            }
        }
    }

    pub(super) fn packet(ident: u16, seq: u16) -> [u8; 64] {
        let mut p = [0u8; 64];
        p[0] = 8;
        p[4..6].copy_from_slice(&ident.to_be_bytes());
        p[6..8].copy_from_slice(&seq.to_be_bytes());
        for (i, b) in p[8..].iter_mut().enumerate() {
            *b = i as u8;
        }
        let c = checksum(&p);
        p[2..4].copy_from_slice(&c.to_be_bytes());
        p
    }

    pub(super) fn checksum(data: &[u8]) -> u16 {
        let mut sum: u32 =
            data.chunks(2).map(|c| u32::from(u16::from_be_bytes([c[0], c.get(1).copied().unwrap_or(0)]))).sum();
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    // (type, identifier, sequence), past an IPv4 header if there is one.
    pub(super) fn parse_reply(b: &[u8]) -> Option<(u8, u16, u16)> {
        let off = match b.first() {
            Some(v) if v >> 4 == 4 => usize::from(v & 0x0f) * 4,
            _ => 0,
        };
        let icmp = b.get(off..)?;
        if icmp.len() < 8 {
            return None;
        }
        Some((icmp[0], u16::from_be_bytes([icmp[4], icmp[5]]), u16::from_be_bytes([icmp[6], icmp[7]])))
    }
}

#[cfg(windows)]
mod sys {
    use super::{Outcome, ms};
    use std::net::Ipv4Addr;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
    };

    pub fn available() -> bool {
        true
    }

    // IcmpSendEcho needs no admin rights.
    pub fn echo(addr: Ipv4Addr, _ident: u16, _seq: u16, timeout: Duration) -> Outcome {
        let data = [0x61u8; 32];
        let size = std::mem::size_of::<ICMP_ECHO_REPLY>() + data.len() + 8 + 64;
        let mut reply = vec![0u64; size.div_ceil(8)];
        let dest = u32::from_ne_bytes(addr.octets());
        let wait = timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
        unsafe {
            let h = IcmpCreateFile();
            if h == INVALID_HANDLE_VALUE {
                return Outcome::Lost;
            }
            let start = Instant::now();
            let n = IcmpSendEcho(
                h,
                dest,
                data.as_ptr().cast(),
                data.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr().cast(),
                (reply.len() * 8) as u32,
                wait,
            );
            let elapsed = ms(start);
            IcmpCloseHandle(h);
            if n == 0 {
                return Outcome::Lost;
            }
            let r = std::ptr::read_unaligned(reply.as_ptr() as *const ICMP_ECHO_REPLY);
            if r.Status != 0 {
                return Outcome::Lost;
            }
            Outcome::Reply(elapsed)
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    use super::Outcome;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    pub fn available() -> bool {
        false
    }

    pub fn echo(_addr: Ipv4Addr, _ident: u16, _seq: u16, _timeout: Duration) -> Outcome {
        Outcome::Lost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(outcomes: &[Outcome]) -> (Series, Instant) {
        let now = Instant::now();
        let mut s = Series::default();
        let n = outcomes.len() as u32;
        for (i, o) in outcomes.iter().enumerate() {
            let sent = now - Duration::from_secs(u64::from(n - i as u32));
            s.begin(i as u32 + 1, sent);
            s.finish(i as u32 + 1, *o);
        }
        (s, now)
    }

    #[test]
    fn summary_counts_loss_and_streak() {
        use Outcome::*;
        let (s, now) = series(&[Reply(10.0), Reply(12.0), Lost, Reply(11.0), Lost, Lost]);
        let sum = s.summary(now);
        assert_eq!(sum.done, 6);
        assert_eq!(sum.replies, 3);
        assert_eq!(sum.lost_streak, 2);
        assert_eq!(sum.last, Some(11.0));
        assert_eq!(sum.median, Some(11.0));
        assert!((sum.loss.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn pending_is_ignored_and_window_applies() {
        let now = Instant::now();
        let mut s = Series::default();
        s.begin(1, now - Duration::from_secs(120));
        s.finish(1, Outcome::Lost);
        s.begin(2, now - Duration::from_secs(1));
        s.finish(2, Outcome::Reply(5.0));
        s.begin(3, now);
        let sum = s.summary(now);
        assert_eq!(sum.done, 1);
        assert_eq!(sum.loss, Some(0.0));
        assert_eq!(s.history(5), vec![None, Some(5.0)]);
    }

    #[cfg(unix)]
    #[test]
    fn icmp_packet_round_trip() {
        let p = sys::packet(0x1234, 7);
        assert_eq!(sys::checksum(&p), 0);
        let mut reply = p;
        reply[0] = 0;
        assert_eq!(sys::parse_reply(&reply), Some((0, 0x1234, 7)));
        let mut with_ip = vec![0x45u8; 20];
        with_ip.extend_from_slice(&reply);
        assert_eq!(sys::parse_reply(&with_ip), Some((0, 0x1234, 7)));
    }
}
