use super::unix::{iface_of, run};
use super::{LinkKind, Local, Wifi, egress_ip};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

pub fn detect() -> Option<Local> {
    let ip = egress_ip()?;
    let name = iface_of(ip)?;
    let routes = fs::read_to_string("/proc/net/route").unwrap_or_default();
    Some(Local {
        gateway: default_gateway(&routes, &name),
        kind: kind_of(&name),
        link_mbps: link_speed(&name),
        key: name.clone(),
        name,
        ip,
    })
}

pub fn counters(local: &Local) -> Option<(u64, u64)> {
    parse_net_dev(&fs::read_to_string("/proc/net/dev").ok()?, &local.key)
}

pub fn wifi(local: &Local) -> Option<Wifi> {
    if let Some(w) = run("iw", &["dev", &local.key, "link"]).and_then(|t| parse_iw_link(&t)) {
        return Some(w);
    }
    let mut w = run(
        "nmcli",
        &["-t", "-f", "ACTIVE,SSID,FREQ,RATE,SIGNAL", "device", "wifi", "list", "--rescan", "no", "ifname", &local.key],
    )
    .and_then(|t| parse_nmcli(&t))
    .unwrap_or_default();
    if let Some(dbm) = fs::read_to_string("/proc/net/wireless").ok().and_then(|t| parse_proc_wireless(&t, &local.key)) {
        w.dbm = Some(dbm);
    }
    (w != Wifi::default()).then_some(w)
}

fn kind_of(iface: &str) -> LinkKind {
    let base = Path::new("/sys/class/net").join(iface);
    if base.join("wireless").exists() || base.join("phy80211").exists() {
        return LinkKind::Wifi;
    }
    let lower = iface.to_ascii_lowercase();
    const VPN: [&str; 12] =
        ["tun", "tap", "wg", "ppp", "tailscale", "zt", "nordlynx", "proton", "mullvad", "ipsec", "vpn", "outline"];
    if VPN.iter().any(|p| lower.starts_with(p)) {
        return LinkKind::Vpn;
    }
    if ["wwan", "wwp", "rmnet", "ccmni"].iter().any(|p| lower.starts_with(p)) {
        return LinkKind::Mobile;
    }
    match fs::read_to_string(base.join("type")).unwrap_or_default().trim() {
        "1" if base.join("device").exists() => LinkKind::Ethernet,
        "65534" | "512" => LinkKind::Vpn,
        _ => LinkKind::Other,
    }
}

fn link_speed(iface: &str) -> Option<u64> {
    let s: i64 = fs::read_to_string(Path::new("/sys/class/net").join(iface).join("speed")).ok()?.trim().parse().ok()?;
    (s > 0).then_some(s as u64)
}

// Default route gateway; the active interface wins, then the lowest metric.
pub(crate) fn default_gateway(routes: &str, prefer: &str) -> Option<Ipv4Addr> {
    let mut best: Option<((bool, u32), Ipv4Addr)> = None;
    for line in routes.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 || f[1] != "00000000" || f[7] != "00000000" {
            continue;
        }
        let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
        let Ok(gw) = u32::from_str_radix(f[2], 16) else { continue };
        if flags & 0x3 != 0x3 || gw == 0 {
            continue;
        }
        let addr = Ipv4Addr::from(gw.to_ne_bytes());
        let rank = (f[0] != prefer, f[6].parse().unwrap_or(u32::MAX));
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, addr));
        }
    }
    best.map(|(_, a)| a)
}

pub(crate) fn parse_net_dev(text: &str, iface: &str) -> Option<(u64, u64)> {
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if name.trim() != iface {
            continue;
        }
        let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if f.len() >= 9 {
            return Some((f[0], f[8]));
        }
    }
    None
}

pub(crate) fn parse_iw_link(text: &str) -> Option<Wifi> {
    if !text.starts_with("Connected") {
        return None;
    }
    let mut w = Wifi::default();
    for line in text.lines().map(str::trim) {
        let first = |v: &str| v.split_whitespace().next().map(str::to_owned);
        if let Some(v) = line.strip_prefix("SSID: ") {
            w.ssid = Some(unescape_iw(v));
        } else if let Some(v) = line.strip_prefix("freq: ") {
            w.freq_mhz = first(v).and_then(|x| x.parse::<f64>().ok()).map(|f| f.round() as u32);
        } else if let Some(v) = line.strip_prefix("signal: ") {
            w.dbm = first(v).and_then(|x| x.parse().ok());
        } else if let Some(v) = line.strip_prefix("tx bitrate: ") {
            w.rate_mbps = first(v).and_then(|x| x.parse().ok());
        }
    }
    Some(w)
}

// iw prints odd SSID bytes as \xNN.
fn unescape_iw(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 2]), hex(b[i + 3])) {
                out.push((h * 16 + l) as u8);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// nmcli terse mode escapes ':' and '\' with a backslash.
fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

pub(crate) fn parse_nmcli(text: &str) -> Option<Wifi> {
    for line in text.lines() {
        let f = split_terse(line);
        if f.len() < 5 || f[0] != "yes" {
            continue;
        }
        let first = |v: &str| v.split_whitespace().next().map(str::to_owned);
        return Some(Wifi {
            ssid: (!f[1].is_empty()).then(|| f[1].clone()),
            freq_mhz: first(&f[2]).and_then(|x| x.parse().ok()),
            rate_mbps: first(&f[3]).and_then(|x| x.parse().ok()),
            dbm: f[4].trim().parse::<i32>().ok().map(|p| p / 2 - 100),
        });
    }
    None
}

pub(crate) fn parse_proc_wireless(text: &str, iface: &str) -> Option<i32> {
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if name.trim() != iface {
            continue;
        }
        let level: f64 = rest.split_whitespace().nth(2)?.trim_end_matches('.').parse().ok()?;
        let level = level as i32;
        return Some(if level > 0 { level - 256 } else { level });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTES: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp0s20f3\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0
";

    #[test]
    fn gateway_prefers_active_interface() {
        assert_eq!(default_gateway(ROUTES, "wlp0s20f3"), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(default_gateway(ROUTES, "wg0"), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(default_gateway("Iface\n", "eth0"), None);
    }

    #[test]
    fn net_dev_counters() {
        let text = "Inter-|   Receive |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets
    lo: 4016702   50008    0    0    0     0          0         0  4016702   50008    0    0    0     0       0          0
wlp0s20f3: 2276377594 2753749    0    0    0     0          0         0 688881847 1431067    0    0    0     0       0          0
";
        assert_eq!(parse_net_dev(text, "wlp0s20f3"), Some((2276377594, 688881847)));
        assert_eq!(parse_net_dev(text, "eth9"), None);
    }

    #[test]
    fn iw_link_output() {
        let text = "Connected to 00:00:5e:00:53:01 (on wlp0s20f3)
\tSSID: Caf\\xc3\\xa9 5G
\tfreq: 5200.0
\tsignal: -59 dBm
\trx bitrate: 1080.6 MBit/s 80MHz HE-MCS 10
\ttx bitrate: 1200.9 MBit/s 80MHz HE-MCS 11
";
        let w = parse_iw_link(text).unwrap();
        assert_eq!(w.ssid.as_deref(), Some("Café 5G"));
        assert_eq!(w.freq_mhz, Some(5200));
        assert_eq!(w.dbm, Some(-59));
        assert_eq!(w.rate_mbps, Some(1200.9));
        assert_eq!(parse_iw_link("Not connected.\n"), None);
    }

    #[test]
    fn nmcli_and_proc_wireless() {
        let w = parse_nmcli("no:Other:2437 MHz:54 Mbit/s:30\nyes:My\\:Net:5200 MHz:540 Mbit/s:72\n").unwrap();
        assert_eq!(w.ssid.as_deref(), Some("My:Net"));
        assert_eq!(w.freq_mhz, Some(5200));
        assert_eq!(w.rate_mbps, Some(540.0));
        assert_eq!(w.dbm, Some(-64));
        let pw = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
wlp0s20f3: 0000   52.  -58.  -256        0      0      0      0   7214        0
";
        assert_eq!(parse_proc_wireless(pw, "wlp0s20f3"), Some(-58));
    }
}
