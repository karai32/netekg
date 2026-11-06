use super::{LinkKind, Local, Wifi, egress_ip};

pub fn detect() -> Option<Local> {
    let ip = egress_ip()?;
    Some(Local { name: "?".into(), key: String::new(), kind: LinkKind::Other, ip, gateway: None, link_mbps: None })
}

pub fn counters(_local: &Local) -> Option<(u64, u64)> {
    None
}

pub fn wifi(_local: &Local) -> Option<Wifi> {
    None
}
