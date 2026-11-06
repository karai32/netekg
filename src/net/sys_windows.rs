use super::{LinkKind, Local, Wifi, egress_ip};
use std::ffi::{CStr, c_void};
use std::net::Ipv4Addr;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, HANDLE, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, GetBestInterface, GetIfEntry2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IF_TYPE_PPP,
    IF_TYPE_PROP_VIRTUAL, IF_TYPE_TUNNEL, IF_TYPE_WWANPP, IP_ADAPTER_ADDRESSES_LH, MIB_IF_ROW2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::NetworkManagement::WiFi::{
    WLAN_BSS_ENTRY, WLAN_BSS_LIST, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
    WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanGetNetworkBssList, WlanOpenHandle, WlanQueryInterface,
    dot11_BSS_type_infrastructure, wlan_interface_state_connected, wlan_intf_opcode_current_connection,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN, SOCKET_ADDRESS};
use windows_sys::core::{GUID, PCWSTR, PSTR};

struct Adapter {
    index: u32,
    name: String,
    description: String,
    guid: String,
    if_type: u32,
    up: bool,
    ips: Vec<Ipv4Addr>,
    gateway: Option<Ipv4Addr>,
    metric: u32,
    speed_bps: u64,
}

fn wide(p: PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn narrow(p: PSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p.cast()).to_string_lossy().into_owned() }
}

fn v4(sa: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    if sa.lpSockaddr.is_null() {
        return None;
    }
    unsafe {
        if (*sa.lpSockaddr).sa_family != AF_INET {
            return None;
        }
        let sin = &*(sa.lpSockaddr as *const SOCKADDR_IN);
        Some(Ipv4Addr::from(sin.sin_addr.S_un.S_addr.to_ne_bytes()))
    }
}

fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 32 * 1024;
    for _ in 0..4 {
        // u64 storage keeps the list properly aligned.
        let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe { GetAdaptersAddresses(u32::from(AF_INET), flags, null(), head, &mut size) };
        if r == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if r != NO_ERROR {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cur = head;
        while !cur.is_null() {
            let a = unsafe { &*cur };
            let mut ips = Vec::new();
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let ua = unsafe { &*u };
                ips.extend(v4(&ua.Address));
                u = ua.Next;
            }
            let mut gateway = None;
            let mut g = a.FirstGatewayAddress;
            while !g.is_null() && gateway.is_none() {
                let ga = unsafe { &*g };
                gateway = v4(&ga.Address).filter(|ip| !ip.is_unspecified());
                g = ga.Next;
            }
            out.push(Adapter {
                index: unsafe { a.Anonymous1.Anonymous.IfIndex },
                name: wide(a.FriendlyName),
                description: wide(a.Description),
                guid: narrow(a.AdapterName),
                if_type: a.IfType,
                up: a.OperStatus == IfOperStatusUp,
                ips,
                gateway,
                metric: a.Ipv4Metric,
                speed_bps: a.TransmitLinkSpeed,
            });
            cur = a.Next;
        }
        return out;
    }
    Vec::new()
}

fn kind_of(a: &Adapter) -> LinkKind {
    let d = a.description.to_ascii_lowercase();
    const VPN: [&str; 8] = ["vpn", "tap-windows", "wintun", "wireguard", "tunnel", "openvpn", "tailscale", "zerotier"];
    if VPN.iter().any(|k| d.contains(k)) {
        return LinkKind::Vpn;
    }
    match a.if_type {
        IF_TYPE_IEEE80211 => LinkKind::Wifi,
        IF_TYPE_ETHERNET_CSMACD => LinkKind::Ethernet,
        IF_TYPE_PPP | IF_TYPE_TUNNEL | IF_TYPE_PROP_VIRTUAL => LinkKind::Vpn,
        t if t == IF_TYPE_WWANPP || t == IF_TYPE_WWANPP + 1 => LinkKind::Mobile,
        _ => LinkKind::Other,
    }
}

pub fn detect() -> Option<Local> {
    let ip = egress_ip()?;
    let list = adapters();
    let mut best = 0u32;
    let found = unsafe { GetBestInterface(u32::from_ne_bytes([1, 1, 1, 1]), &mut best) } == NO_ERROR;
    let a = list.iter().find(|a| found && a.index == best).or_else(|| list.iter().find(|a| a.ips.contains(&ip)))?;
    // VPN adapters often lack a gateway; use the physical one's.
    let gateway = a.gateway.or_else(|| {
        list.iter().filter(|x| x.up && x.gateway.is_some()).min_by_key(|x| x.metric).and_then(|x| x.gateway)
    });
    let kind = kind_of(a);
    let link_mbps =
        (kind == LinkKind::Ethernet && a.speed_bps > 0 && a.speed_bps != u64::MAX).then_some(a.speed_bps / 1_000_000);
    Some(Local {
        name: if a.name.is_empty() { a.description.clone() } else { a.name.clone() },
        key: format!("{} {}", a.index, a.guid),
        kind,
        ip,
        gateway,
        link_mbps,
    })
}

pub fn counters(local: &Local) -> Option<(u64, u64)> {
    let index: u32 = local.key.split_whitespace().next()?.parse().ok()?;
    let mut row = MIB_IF_ROW2 { InterfaceIndex: index, ..Default::default() };
    (unsafe { GetIfEntry2(&mut row) } == NO_ERROR).then_some((row.InOctets, row.OutOctets))
}

fn guid_text(g: &GUID) -> String {
    let d = g.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

// Wi-Fi details from the WLAN API; Windows 11 may need location access.
pub fn wifi(local: &Local) -> Option<Wifi> {
    let guid = local.key.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut version = 0u32;
    let mut h: HANDLE = null_mut();
    if unsafe { WlanOpenHandle(2, null(), &mut version, &mut h) } != 0 {
        return None;
    }
    let result = query(h, &guid);
    unsafe { WlanCloseHandle(h, null()) };
    result
}

fn query(h: HANDLE, guid: &str) -> Option<Wifi> {
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
    if unsafe { WlanEnumInterfaces(h, null(), &mut list) } != 0 || list.is_null() {
        return None;
    }
    let iface = unsafe {
        let n = (*list).dwNumberOfItems as usize;
        // The trailing one-element array really holds n items.
        let first = (&raw const (*list).InterfaceInfo).cast::<WLAN_INTERFACE_INFO>();
        let items = std::slice::from_raw_parts(first, n);
        items
            .iter()
            .find(|i| guid_text(&i.InterfaceGuid).eq_ignore_ascii_case(guid))
            .or_else(|| items.iter().find(|i| i.isState == wlan_interface_state_connected))
            .map(|i| i.InterfaceGuid)
    };
    unsafe { WlanFreeMemory(list as *const c_void) };
    connection(h, &iface?)
}

fn connection(h: HANDLE, iface: &GUID) -> Option<Wifi> {
    let mut size = 0u32;
    let mut data: *mut c_void = null_mut();
    let r = unsafe {
        WlanQueryInterface(h, iface, wlan_intf_opcode_current_connection, null(), &mut size, &mut data, null_mut())
    };
    if r != 0 || data.is_null() {
        return None;
    }
    let (mut wifi, bssid) = unsafe {
        let c = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
        let a = &c.wlanAssociationAttributes;
        let len = (a.dot11Ssid.uSSIDLength as usize).min(32);
        let ssid = String::from_utf8_lossy(&a.dot11Ssid.ucSSID[..len]).into_owned();
        let rate = f64::from(a.ulTxRate.max(a.ulRxRate)) / 1000.0;
        let wifi = Wifi {
            ssid: (!ssid.is_empty()).then_some(ssid),
            dbm: Some(a.wlanSignalQuality.min(100) as i32 / 2 - 100),
            freq_mhz: None,
            rate_mbps: (rate > 0.0).then_some(rate),
        };
        (wifi, a.dot11Bssid)
    };
    unsafe { WlanFreeMemory(data) };
    // The access point entry has the real RSSI and the channel frequency.
    let mut bss: *mut WLAN_BSS_LIST = null_mut();
    let r = unsafe { WlanGetNetworkBssList(h, iface, null(), dot11_BSS_type_infrastructure, 0, null(), &mut bss) };
    if r == 0 && !bss.is_null() {
        unsafe {
            let n = (*bss).dwNumberOfItems as usize;
            let first = (&raw const (*bss).wlanBssEntries).cast::<WLAN_BSS_ENTRY>();
            let entries = std::slice::from_raw_parts(first, n);
            if let Some(e) = entries.iter().find(|e| e.dot11Bssid == bssid) {
                wifi.dbm = Some(e.lRssi);
                wifi.freq_mhz = Some(e.ulChCenterFrequency / 1000);
            }
            WlanFreeMemory(bss as *const c_void);
        }
    }
    Some(wifi)
}
