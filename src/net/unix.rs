use std::ffi::CStr;
use std::net::Ipv4Addr;

// All IPv4 addresses with their interface names.
pub fn ipv4_ifaces() -> Vec<(String, Ipv4Addr)> {
    let mut out = Vec::new();
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut p = head;
        while !p.is_null() {
            let ifa = &*p;
            if !ifa.ifa_addr.is_null() && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                out.push((name, ip));
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

pub fn iface_of(ip: Ipv4Addr) -> Option<String> {
    ipv4_ifaces().into_iter().find(|(_, a)| *a == ip).map(|(n, _)| n)
}

// Runs a helper tool with a C locale and returns its stdout on success.
pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
