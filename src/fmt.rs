// Numbers and units for the interface and the command line.

pub fn num(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

// About three significant digits: 0.85, 8.42, 84.2, 842.
pub fn auto(v: f64) -> String {
    let a = v.abs();
    let d = if a < 0.005 {
        0
    } else if a < 10.0 {
        2
    } else if a < 100.0 {
        1
    } else {
        0
    };
    num(v, d)
}

// Splits a rate into number and unit, switching units at 1000.
pub fn bits_parts(bps: f64) -> (String, &'static str) {
    if bps >= 1e9 {
        (auto(bps / 1e9), "Gbps")
    } else if bps >= 1e6 {
        (auto(bps / 1e6), "Mbps")
    } else {
        (auto(bps / 1e3), "kbps")
    }
}

pub fn bits(bps: f64) -> String {
    let (n, unit) = bits_parts(bps);
    format!("{n} {unit}")
}

// Without trailing zeros: 1 Gbps, 2.5 Mbps.
pub fn bits_short(bps: f64) -> String {
    let (n, unit) = bits_parts(bps);
    let n = if n.contains('.') { n.trim_end_matches('0').trim_end_matches('.') } else { &n };
    format!("{n} {unit}")
}

pub fn mbit(mbps: f64) -> String {
    bits(mbps * 1e6)
}

pub fn bytes(b: f64) -> String {
    let (v, unit) = if b >= 1e9 {
        (b / 1e9, "GB")
    } else if b >= 1e6 {
        (b / 1e6, "MB")
    } else if b >= 1e3 {
        (b / 1e3, "kB")
    } else {
        return format!("{} B", b.round());
    };
    format!("{} {unit}", auto(v))
}

pub fn byte_rate(bps: f64) -> String {
    let b = bps / 8.0;
    let (v, unit) = if b >= 1e9 {
        (b / 1e9, "GB/s")
    } else if b >= 1e6 {
        (b / 1e6, "MB/s")
    } else {
        (b / 1e3, "kB/s")
    };
    format!("{} {unit}", auto(v))
}

pub fn ms(v: f64) -> String {
    if v < 10.0 { format!("{} ms", num(v, 1)) } else { format!("{} ms", num(v.round(), 0)) }
}

pub fn pct(v: f64) -> String {
    if v <= 0.0 {
        "0%".into()
    } else if v < 1.0 {
        format!("{}%", num(v, 1))
    } else {
        format!("{}%", num(v.round(), 0))
    }
}

pub fn ago(secs: u64) -> String {
    match secs {
        0..5 => "just now".into(),
        5..60 => format!("{secs} s ago"),
        60..3600 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(bits(0.0), "0 kbps");
        assert_eq!(bits(3_200.0), "3.20 kbps");
        assert_eq!(bits(12_400_000.0), "12.4 Mbps");
        assert_eq!(bits(314_000_000.0), "314 Mbps");
        assert_eq!(bits(1_250_000_000.0), "1.25 Gbps");
        assert_eq!(bits_parts(92_400_000.0), ("92.4".to_owned(), "Mbps"));
        assert_eq!(bits_short(1e9), "1 Gbps");
        assert_eq!(bits_short(2.5e6), "2.5 Mbps");
        assert_eq!(bits_short(200e3), "200 kbps");
    }

    #[test]
    fn sizes_and_byte_rates() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1_300_000_000.0), "1.30 GB");
        assert_eq!(byte_rate(12_400_000.0), "1.55 MB/s");
        assert_eq!(byte_rate(800.0), "0.10 kB/s");
    }

    #[test]
    fn times_and_percent() {
        assert_eq!(ms(3.44), "3.4 ms");
        assert_eq!(ms(18.6), "19 ms");
        assert_eq!(pct(0.0), "0%");
        assert_eq!(pct(0.5), "0.5%");
        assert_eq!(pct(12.4), "12%");
        assert_eq!(ago(2), "just now");
        assert_eq!(ago(42), "42 s ago");
        assert_eq!(ago(125), "2 min ago");
    }
}
