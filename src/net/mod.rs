// Local network facts: active interface, router, traffic counters and Wi-Fi.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod sys_linux;
#[cfg(target_os = "linux")]
use sys_linux as sys;

#[cfg(target_os = "macos")]
mod sys_macos;
#[cfg(target_os = "macos")]
use sys_macos as sys;

#[cfg(windows)]
mod sys_windows;
#[cfg(windows)]
use sys_windows as sys;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod sys_other;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
use sys_other as sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Wifi,
    Ethernet,
    Vpn,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Mobile,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    // Counter handle: interface name on Unix, index on Windows.
    pub key: String,
    pub kind: LinkKind,
    pub ip: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub link_mbps: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub ssid: Option<String>,
    pub dbm: Option<i32>,
    pub freq_mhz: Option<u32>,
    pub rate_mbps: Option<f64>,
}

impl Wifi {
    pub fn band_ghz(&self) -> Option<f64> {
        let f = self.freq_mhz?;
        Some(if f < 3000 {
            2.4
        } else if f < 5925 {
            5.0
        } else {
            6.0
        })
    }

    pub fn channel(&self) -> Option<u32> {
        channel_of(self.freq_mhz?)
    }

    // 0 excellent, 1 good, 2 fair, 3 weak.
    pub fn level(&self) -> Option<usize> {
        let d = self.dbm?;
        Some(if d >= -55 {
            0
        } else if d >= -67 {
            1
        } else if d >= -75 {
            2
        } else {
            3
        })
    }
}

pub fn channel_of(freq: u32) -> Option<u32> {
    match freq {
        2412..=2472 => Some((freq - 2407) / 5),
        2484 => Some(14),
        5955..=7115 => Some((freq - 5950) / 5),
        5000..=5925 => Some((freq - 5000) / 5),
        _ => None,
    }
}

// Local address used to reach the internet; sends nothing.
pub fn egress_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddr::from(([1, 1, 1, 1], 53))).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_unspecified() => Some(*a.ip()),
        _ => None,
    }
}

pub fn detect() -> Option<Local> {
    sys::detect()
}

pub fn counters(local: &Local) -> Option<(u64, u64)> {
    sys::counters(local)
}

pub fn wifi(local: &Local) -> Option<Wifi> {
    if local.kind != LinkKind::Wifi {
        return None;
    }
    sys::wifi(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels() {
        assert_eq!(channel_of(2412), Some(1));
        assert_eq!(channel_of(2484), Some(14));
        assert_eq!(channel_of(5200), Some(40));
        assert_eq!(channel_of(5955), Some(1));
        assert_eq!(channel_of(900), None);
    }

    #[test]
    fn bands_and_levels() {
        let w = Wifi { freq_mhz: Some(5200), dbm: Some(-59), ..Default::default() };
        assert_eq!(w.band_ghz(), Some(5.0));
        assert_eq!(w.level(), Some(1));
        let w = Wifi { freq_mhz: Some(2437), dbm: Some(-80), ..Default::default() };
        assert_eq!(w.band_ghz(), Some(2.4));
        assert_eq!(w.level(), Some(3));
    }
}
