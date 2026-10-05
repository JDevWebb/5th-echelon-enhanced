//! What a server says about itself on port 80 (`GET /api/info`), the one
//! port that never moves because the game always uses it.
//!
//! A server behind a reverse proxy, or with its ports forwarded to other
//! numbers, reports the ports players use there. Set up reads them, so
//! players only ever type the server's name.
//!
//! The same answer comes over HTTPS from a server with a certificate (the
//! launcher asks there first, see its `flow`): only that one can be trusted
//! not to have been changed on the way, so only that one may name a
//! coordinator or mark a server as having HTTPS.

use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

/// The ports players use on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Ports {
    pub api: u16,
    pub login: u16,
    /// None when the server has no NAT helper (LAN or VPN play only).
    #[serde(default)]
    pub nat: Option<u16>,
    /// The API over HTTPS, when the server has it (a domain behind the
    /// installer's Caddy): used instead of `api`, so nothing travels readable.
    #[serde(default)]
    pub api_tls: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub version: String,
    /// Missing on servers before this was added: they use the default ports.
    #[serde(default)]
    pub ports: Option<Ports>,
    /// The server's id, which players sign into identity links (servers
    /// with friend lists).
    #[serde(default)]
    pub id: Option<String>,
    /// "everyone" or "mutual" (see the server's `[friends]`).
    #[serde(default)]
    pub friends_mode: Option<String>,
    /// The coordinator the server shares friends with.
    #[serde(default)]
    pub coordinator: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

/// Asks `server` (an IP or host name, as typed) for `/api/info`. None when
/// it doesn't answer, or has the API switched off.
pub fn fetch(server: &str, timeout: Duration) -> Option<ServerInfo> {
    fetch_on(server, crate::CONFIG_PORT, timeout)
}

fn fetch_on(server: &str, port: u16, timeout: Duration) -> Option<ServerInfo> {
    parse_json(&get(server, port, "/api/info", timeout)?)
}

/// One item of a server's news (`GET /api/news`, what the game's news screen
/// shows). Text from the server: show it as text, and only open a link the
/// player clicks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewsItem {
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub link: String,
}

/// The server's news; empty when it has none or doesn't say.
pub fn news(server: &str, timeout: Duration) -> Vec<NewsItem> {
    #[derive(Deserialize)]
    struct News {
        news: Vec<NewsItem>,
    }
    get(server, crate::CONFIG_PORT, "/api/news", timeout)
        .and_then(|body| serde_json::from_str::<News>(&body).ok())
        .map(|n| n.news.into_iter().take(10).collect())
        .unwrap_or_default()
}

/// The body of a GET to `path` on `server`'s port `port`, if it answers 200.
fn get(server: &str, port: u16, path: &str, timeout: Duration) -> Option<String> {
    let host = server.trim();
    if !crate::net::valid_host(host) {
        return None;
    }
    let addr: SocketAddr = (host, port).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    // The Host header lets a reverse proxy route by name.
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.take(64 * 1024).read_to_end(&mut response).ok()?;
    response_body(&response)
}

#[cfg(test)]
fn parse_response(response: &[u8]) -> Option<ServerInfo> {
    parse_json(&response_body(response)?)
}

/// The body of a 200 answer, however it arrived.
fn response_body(response: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(response).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?;
    if !status.split_whitespace().nth(1).is_some_and(|code| code == "200") {
        return None;
    }
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if chunked {
        dechunk(body)
    } else {
        Some(body.to_string())
    }
}

/// The answer's body, however it arrived (the launcher reads the HTTPS one itself).
pub fn parse_json(body: &str) -> Option<ServerInfo> {
    serde_json::from_str(body).ok()
}

/// A chunked body, joined (a proxy may send the answer that way).
fn dechunk(mut body: &str) -> Option<String> {
    let mut out = String::new();
    loop {
        let (size, rest) = body.split_once("\r\n")?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        if size == 0 {
            return Some(out);
        }
        out.push_str(rest.get(..size)?);
        body = rest.get(size..)?.strip_prefix("\r\n")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ports() {
        let body = r#"{"name":"5th Echelon Enhanced","version":"0.3.0","features":[],"ports":{"api":80,"login":31126,"secure":31127,"content":80,"nat":31128}}"#;
        let plain = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let info = parse_response(plain.as_bytes()).unwrap();
        assert_eq!(
            info.ports,
            Some(Ports {
                api: 80,
                login: 31126,
                nat: Some(31128),
                api_tls: None
            })
        );

        let (a, b) = body.split_at(20);
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
            a.len(),
            b.len()
        );
        assert_eq!(parse_response(chunked.as_bytes()), Some(info));
    }

    #[test]
    fn older_servers_and_errors() {
        let old = "HTTP/1.1 200 OK\r\n\r\n{\"version\":\"0.2.5\"}";
        assert_eq!(parse_response(old.as_bytes()).unwrap().ports, None);
        let nat_off = "HTTP/1.1 200 OK\r\n\r\n{\"ports\":{\"api\":50051,\"login\":21126,\"secure\":21127,\"content\":8000,\"nat\":null}}";
        assert_eq!(parse_response(nat_off.as_bytes()).unwrap().ports.unwrap().nat, None);
        assert_eq!(parse_response(b"HTTP/1.1 404 Not Found\r\n\r\n"), None);
        // The game's online config (the API switched off answers it instead).
        assert_eq!(parse_response(b"HTTP/1.1 200 OK\r\n\r\n[{\"Name\":\"SandboxUrl\"}]"), None);
    }

    #[test]
    fn asks_by_name() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let mut n = 0;
            while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                n += s.read(&mut buf[n..]).unwrap();
            }
            let body = r#"{"ports":{"api":50051,"login":21126,"nat":21128}}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let info = fetch_on("localhost", port, Duration::from_secs(2)).unwrap();
        assert_eq!(info.ports.unwrap().nat, Some(21128));
        assert!(server.join().unwrap().starts_with("GET /api/info HTTP/1.1\r\nHost: localhost\r\n"));
    }
}
