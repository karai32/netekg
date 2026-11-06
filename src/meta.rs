// Public IP, provider and the nearest data center, as Cloudflare sees them.

use crate::http;
use serde_json::Value;
use std::io;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PublicInfo {
    pub ip: String,
    pub isp: Option<String>,
    pub colo: Option<String>,
    pub colo_city: Option<String>,
}

pub fn fetch() -> io::Result<PublicInfo> {
    let (head, body) = http::get_text(http::HOST, "/meta", &[("Referer", "https://speed.cloudflare.com/")])?;
    if head.status != 200 {
        return Err(io::Error::other(format!("HTTP {}", head.status)));
    }
    parse(&body).ok_or_else(|| io::Error::other("unexpected answer"))
}

pub fn parse(body: &str) -> Option<PublicInfo> {
    let v: Value = serde_json::from_str(body).ok()?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let colo = v.get("colo");
    Some(PublicInfo {
        ip: s(&v, "clientIp")?,
        isp: s(&v, "asOrganization"),
        colo: colo.and_then(|c| s(c, "iata").or_else(|| c.as_str().map(str::to_owned))),
        colo_city: colo.and_then(|c| s(c, "city")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta() {
        let body = r#"{"hostname":"speed.cloudflare.com","clientIp":"203.0.113.7","httpProtocol":"HTTP/1.1","asn":64500,"asOrganization":"Example Telecom","country":"NL","city":"Amsterdam","colo":{"iata":"AMS","city":"Amsterdam"}}"#;
        let p = parse(body).unwrap();
        assert_eq!(p.ip, "203.0.113.7");
        assert_eq!(p.isp.as_deref(), Some("Example Telecom"));
        assert_eq!(p.colo.as_deref(), Some("AMS"));
        assert_eq!(p.colo_city.as_deref(), Some("Amsterdam"));
        assert_eq!(parse("{}"), None);
    }
}
