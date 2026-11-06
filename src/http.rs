// Minimal HTTP/1.1 client over TLS for the speed test.

use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const HOST: &str = "speed.cloudflare.com";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(8);
const UA: &str = concat!("netekg/", env!("CARGO_PKG_VERSION"));

pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

pub struct Conn {
    io: BufReader<Box<dyn Stream>>,
    host: String,
}

pub struct Head {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn length(&self) -> Option<u64> {
        self.get("content-length")?.trim().parse().ok()
    }

    fn chunked(&self) -> bool {
        self.get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    }

    pub fn keep_alive(&self) -> bool {
        !self.get("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"))
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.to_owned())
}

impl Conn {
    pub fn open(host: &str, timeout: Duration) -> io::Result<Conn> {
        let mut last = io::Error::new(ErrorKind::NotFound, "no address");
        for addr in (host, 443).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(tcp) => {
                    tcp.set_nodelay(true)?;
                    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
                    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
                    let tls = tls::wrap(host, tcp)?;
                    return Ok(Conn { io: BufReader::with_capacity(64 * 1024, tls), host: host.to_owned() });
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn send_head(
        &mut self,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
        length: Option<u64>,
    ) -> io::Result<()> {
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {UA}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: keep-alive\r\n",
            self.host
        );
        for (k, v) in extra {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(n) = length {
            req.push_str(&format!("Content-Length: {n}\r\n"));
        }
        req.push_str("\r\n");
        let w = self.io.get_mut();
        w.write_all(req.as_bytes())?;
        if length.is_none() {
            w.flush()?;
        }
        Ok(())
    }

    pub fn write_body(&mut self, data: &[u8]) -> io::Result<()> {
        self.io.get_mut().write_all(data)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.io.get_mut().flush()
    }

    pub fn read_head(&mut self) -> io::Result<Head> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.io.read_line(&mut line)? == 0 {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            if !line.trim().is_empty() {
                break;
            }
        }
        let status =
            line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| bad("bad status line"))?;
        let mut headers = Vec::new();
        loop {
            line.clear();
            if self.io.read_line(&mut line)? == 0 {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
            if headers.len() > 200 {
                return Err(bad("too many headers"));
            }
        }
        Ok(Head { status, headers })
    }

    // Ok(false) when the sink stopped early.
    pub fn read_body(&mut self, head: &Head, sink: &mut dyn FnMut(&[u8]) -> bool) -> io::Result<bool> {
        let mut buf = vec![0u8; 256 * 1024];
        if head.chunked() {
            let mut line = String::new();
            loop {
                line.clear();
                self.io.read_line(&mut line)?;
                let size = line.trim().split(';').next().unwrap_or("").trim();
                let size = u64::from_str_radix(size, 16).map_err(|_| bad("bad chunk size"))?;
                if size == 0 {
                    loop {
                        line.clear();
                        if self.io.read_line(&mut line)? == 0 || line.trim().is_empty() {
                            return Ok(true);
                        }
                    }
                }
                if !self.copy(size, &mut buf, sink)? {
                    return Ok(false);
                }
                line.clear();
                self.io.read_line(&mut line)?;
            }
        } else if let Some(n) = head.length() {
            self.copy(n, &mut buf, sink)
        } else {
            loop {
                let n = match self.io.read(&mut buf) {
                    Ok(0) => return Ok(true),
                    Ok(n) => n,
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(true),
                    Err(e) => return Err(e),
                };
                if !sink(&buf[..n]) {
                    return Ok(false);
                }
            }
        }
    }

    fn copy(&mut self, mut left: u64, buf: &mut [u8], sink: &mut dyn FnMut(&[u8]) -> bool) -> io::Result<bool> {
        while left > 0 {
            let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
            let n = self.io.read(&mut buf[..want])?;
            if n == 0 {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            left -= n as u64;
            if !sink(&buf[..n]) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub fn get_text(host: &str, path: &str, extra: &[(&str, &str)]) -> io::Result<(Head, String)> {
    let mut c = Conn::open(host, CONNECT_TIMEOUT)?;
    c.send_head("GET", path, extra, None)?;
    let head = c.read_head()?;
    let mut body = Vec::new();
    c.read_body(&head, &mut |b| {
        body.extend_from_slice(b);
        body.len() < 1 << 20
    })?;
    Ok((head, String::from_utf8_lossy(&body).into_owned()))
}

#[cfg(not(any(windows, target_os = "macos")))]
mod tls {
    use super::Stream;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
    use std::io;
    use std::net::TcpStream;
    use std::sync::{Arc, OnceLock};

    fn config() -> io::Result<Arc<ClientConfig>> {
        static CFG: OnceLock<Option<Arc<ClientConfig>>> = OnceLock::new();
        CFG.get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let cfg = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .ok()?
                .with_root_certificates(roots)
                .with_no_client_auth();
            Some(Arc::new(cfg))
        })
        .clone()
        .ok_or_else(|| io::Error::other("TLS setup failed"))
    }

    pub fn wrap(host: &str, tcp: TcpStream) -> io::Result<Box<dyn Stream>> {
        let name = ServerName::try_from(host.to_owned()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let conn = ClientConnection::new(config()?, name).map_err(io::Error::other)?;
        Ok(Box::new(StreamOwned::new(conn, tcp)))
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod tls {
    use super::Stream;
    use std::io;
    use std::net::TcpStream;

    // System TLS on Windows and macOS.
    pub fn wrap(host: &str, tcp: TcpStream) -> io::Result<Box<dyn Stream>> {
        let connector = native_tls::TlsConnector::new().map_err(io::Error::other)?;
        let stream = connector.connect(host, tcp).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Box::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Fake(Cursor<Vec<u8>>);
    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn conn(raw: &str) -> Conn {
        let s: Box<dyn Stream> = Box::new(Fake(Cursor::new(raw.as_bytes().to_vec())));
        Conn { io: BufReader::new(s), host: "x".into() }
    }

    fn body(c: &mut Conn) -> (Head, String) {
        let h = c.read_head().unwrap();
        let mut out = Vec::new();
        assert!(
            c.read_body(&h, &mut |b| {
                out.extend_from_slice(b);
                true
            })
            .unwrap()
        );
        (h, String::from_utf8(out).unwrap())
    }

    #[test]
    fn content_length_and_keep_alive() {
        let mut c = conn(
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\ncolo: BCN\r\n\r\nhelloHTTP/1.1 204 No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let (h, b) = body(&mut c);
        assert_eq!((h.status, b.as_str(), h.get("COLO")), (200, "hello", Some("BCN")));
        assert!(h.keep_alive());
        let (h, b) = body(&mut c);
        assert_eq!((h.status, b.as_str()), (204, ""));
        assert!(!h.keep_alive());
    }

    #[test]
    fn chunked_body() {
        let mut c =
            conn("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n6;x=y\r\npedia \r\n0\r\n\r\n");
        assert_eq!(body(&mut c).1, "Wikipedia ");
    }

    #[test]
    fn sink_can_stop() {
        let mut c = conn("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc");
        let h = c.read_head().unwrap();
        assert!(!c.read_body(&h, &mut |_| false).unwrap());
    }
}
