use super::unix::{iface_of, run};
use super::{LinkKind, Local, Wifi, egress_ip};
use std::ffi::CStr;
use std::net::Ipv4Addr;
use std::sync::OnceLock;

pub fn detect() -> Option<Local> {
    let ip = egress_ip()?;
    let name = iface_of(ip)?;
    let gateway = run("route", &["-n", "get", "default"]).and_then(|t| parse_route_get(&t));
    let kind = if ["utun", "ipsec", "ppp", "tun", "tap"].iter().any(|p| name.starts_with(p)) {
        LinkKind::Vpn
    } else if wifi_device().is_some_and(|d| d == name) {
        LinkKind::Wifi
    } else if name.starts_with("en") {
        LinkKind::Ethernet
    } else {
        LinkKind::Other
    };
    Some(Local { key: name.clone(), name, kind, ip, gateway, link_mbps: None })
}

// Byte counters from the AF_LINK entry, 32-bit on macOS.
pub fn counters(local: &Local) -> Option<(u64, u64)> {
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return None;
        }
        let mut found = None;
        let mut p = head;
        while !p.is_null() {
            let ifa = &*p;
            if !ifa.ifa_addr.is_null()
                && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_LINK
                && !ifa.ifa_data.is_null()
                && CStr::from_ptr(ifa.ifa_name).to_bytes() == local.key.as_bytes()
            {
                let d = &*(ifa.ifa_data as *const libc::if_data);
                found = Some((u64::from(d.ifi_ibytes), u64::from(d.ifi_obytes)));
                break;
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
        found
    }
}

// macOS has no tool that reports Wi-Fi details without root.
pub fn wifi(_local: &Local) -> Option<Wifi> {
    None
}

fn wifi_device() -> Option<&'static str> {
    static DEV: OnceLock<Option<String>> = OnceLock::new();
    DEV.get_or_init(|| {
        let text = run("networksetup", &["-listallhardwareports"])?;
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if line.contains("Wi-Fi") || line.contains("AirPort") {
                return lines.next()?.strip_prefix("Device: ").map(|d| d.trim().to_owned());
            }
        }
        None
    })
    .as_deref()
}

fn parse_route_get(text: &str) -> Option<Ipv4Addr> {
    text.lines().find_map(|l| l.trim().strip_prefix("gateway:")).and_then(|v| v.trim().parse().ok())
}
