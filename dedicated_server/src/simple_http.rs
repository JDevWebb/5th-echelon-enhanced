use std::collections::HashMap;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use slog::debug;
use slog::error;
use slog::warn;

/// How long a client may take to send its request or read the response.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a POST body may take once its headers are in (at most 16 KiB:
/// nobody needs long).
const BODY_DEADLINE: Duration = Duration::from_secs(5);
/// Longest request line or header accepted (the game only ever sends short GETs).
const MAX_REQUEST_LINE: u64 = 8 * 1024;
/// Largest POST body accepted (the community API's small JSON requests).
const MAX_BODY: usize = 16 * 1024;

/// A request: the raw request line (as upstream matched on it), its method
/// and path, for POST the body, and the client's address.
pub struct Request {
    pub line: String,
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    /// The client: the connection's address, or the one a trusted proxy names.
    pub peer: Option<std::net::IpAddr>,
    /// The connection's own address (a proxy's, when there is one).
    pub conn_peer: Option<std::net::IpAddr>,
    /// For POST, the `X-Forwarded-Proto` header (believed from trusted proxies only).
    pub forwarded_proto: Option<String>,
}

impl Request {
    /// Whether a password may be taken in this request (see
    /// `rate_limit::credentials_allowed`).
    pub fn credentials_allowed(&self) -> bool {
        crate::rate_limit::credentials_allowed(self.conn_peer, self.forwarded_proto.as_deref())
    }
}

impl Request {
    fn parse_line(line: &str) -> Self {
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        Self {
            line: line.to_string(),
            method,
            path,
            body: vec![],
            peer: None,
            conn_peer: None,
            forwarded_proto: None,
        }
    }
}

/// A response: status line, content type and body.
pub struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/octet-stream",
            body,
        }
    }

    pub fn status(status: &'static str) -> Self {
        Self {
            status,
            content_type: "",
            body: vec![],
        }
    }

    pub fn json(status: &'static str, value: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    pub fn status_is(&self, status: &str) -> bool {
        self.status == status
    }

    #[cfg(test)]
    pub fn to_bytes_for_test(&self) -> Vec<u8> {
        self.to_bytes()
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = if self.content_type.is_empty() {
            format!("HTTP/1.0 {}\r\n\r\n", self.status).into_bytes()
        } else {
            format!(
                "HTTP/1.0 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                self.status,
                self.content_type,
                self.body.len()
            )
            .into_bytes()
        };
        out.extend_from_slice(&self.body);
        out
    }
}

/// A handler for extra routes; `None` means "not mine".
pub type Routes = Arc<dyn Fn(&Request) -> Option<Response> + Send + Sync>;

/// Serves the online config over HTTP: the given content for any request,
/// except what `routes` answers (the community API). POSTs are read only for
/// the paths `takes_body` names; others are refused before their body.
pub fn serve(logger: &slog::Logger, addr: SocketAddr, content: &str, routes: Option<Routes>, takes_body: fn(&str) -> bool) -> std::io::Result<()> {
    let content = content.as_bytes().to_vec();
    serve_listener(logger, TcpListener::bind(addr)?, takes_body, move |req| {
        routes.as_ref().and_then(|r| r(req)).unwrap_or_else(|| Response::ok(content.clone()))
    })
}

/// Serves multiple files over HTTP.
///
/// This function binds to the given address and serves files from the provided
/// `files` map. The keys of the map are the request paths, and the values are
/// the paths to the files on disk.
pub fn serve_many(logger: &slog::Logger, addr: SocketAddr, files: &HashMap<String, PathBuf>) -> std::io::Result<()> {
    let files = files.clone();
    let log = logger.clone();
    serve_listener(
        logger,
        TcpListener::bind(addr)?,
        |_| false,
        move |req| {
            let line = req.line.as_str();
            let prefix = "GET ";
            let suffix = " HTTP/1.1\r\n";
            if !line.starts_with(prefix) {
                return Response::status("405 Method Not Allowed");
            }
            if !line.ends_with(suffix) {
                return Response::status("400 Bad Request");
            }
            let path = &line[prefix.len()..(line.len() - suffix.len())];
            match files.get(path).map(std::fs::read) {
                Some(Ok(data)) => Response::ok(data),
                Some(Err(e)) => {
                    error!(log, "simple_http: reading {path}: {e}");
                    Response::status("500 Internal Server Error")
                }
                None => Response::status("404 Not Found"),
            }
        },
    )
}

/// Accepts connections forever, answering each on its own thread, so one slow
/// or idle client can't hold up everyone else (upstream served one connection
/// at a time without timeouts).
fn serve_listener<H>(logger: &slog::Logger, listener: TcpListener, takes_body: fn(&str) -> bool, handler: H) -> std::io::Result<()>
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let open = Arc::new(Open::default());
    loop {
        let stream = match listener.accept() {
            Ok((stream, _addr)) => stream,
            Err(e) => {
                // E.g. out of file descriptors: back off instead of stopping.
                warn!(logger, "simple_http: accept failed: {e}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        // Bounded: a thread per connection, so a limit on them, in all and per address
        // (a client holding connections open can't take them all). Everything through a
        // reverse proxy comes from the proxy's address, so there the per-address count
        // waits for the client's address in the headers (see `handle`).
        let ip = stream.peer_addr().map(|a| a.ip()).ok();
        let proxied = ip.is_some_and(crate::rate_limit::is_proxy);
        let Some(slot) = open.take(ip.filter(|_| !proxied), true) else {
            debug!(logger, "simple_http: too many connections; refusing one");
            continue;
        };
        let logger = logger.clone();
        let handler = Arc::clone(&handler);
        let open = Arc::clone(&open);
        std::thread::spawn(move || {
            let _slot = slot;
            let client_slot = |client: Option<std::net::IpAddr>| match client {
                Some(client) if proxied => open.take(Some(client), false).map(Some),
                _ => Some(None),
            };
            if let Err(e) = handle(&logger, stream, takes_body, client_slot, &*handler) {
                debug!(logger, "simple_http: {e}");
            }
        });
    }
}

/// Connections open now, in all and per address.
#[derive(Default)]
struct Open {
    all: std::sync::Mutex<(usize, std::collections::HashMap<std::net::IpAddr, usize>)>,
}

/// Most connections at once, and from one address.
const MAX_OPEN: usize = 256;
const MAX_OPEN_PER_IP: usize = 16;
/// How long one request's line and headers may take in all (each read has
/// its own timeout too).
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

impl Open {
    /// A place for a connection from `ip` (per address; IPv6 by /64), and
    /// with `total` one of the [`MAX_OPEN`] (a proxied client's second
    /// place, in its own name, doesn't count there again).
    fn take(self: &Arc<Self>, ip: Option<std::net::IpAddr>, total: bool) -> Option<Slot> {
        let ip = ip.map(crate::rate_limit::bucket_of);
        let mut guard = self.all.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (open, per_ip) = &mut *guard;
        let mine = ip.map_or(0, |ip| per_ip.get(&ip).copied().unwrap_or(0));
        if (total && *open >= MAX_OPEN) || mine >= MAX_OPEN_PER_IP {
            return None;
        }
        if total {
            *open += 1;
        }
        if let Some(ip) = ip {
            *per_ip.entry(ip).or_default() += 1;
        }
        Some(Slot {
            open: Arc::clone(self),
            ip,
            total,
        })
    }
}

/// One connection's place, given back when it ends.
struct Slot {
    open: Arc<Open>,
    ip: Option<std::net::IpAddr>,
    total: bool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut guard = self.open.all.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (open, per_ip) = &mut *guard;
        if self.total {
            *open = open.saturating_sub(1);
        }
        if let Some(ip) = self.ip {
            if let Some(n) = per_ip.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    per_ip.remove(&ip);
                }
            }
        }
    }
}

/// A reader that gives up once a deadline passes, so a client sending a
/// byte now and then can't keep a connection for hours.
struct Deadline<R> {
    inner: R,
    until: std::time::Instant,
}

impl<R: Read> Read for Deadline<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::time::Instant::now() >= self.until {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request took too long"));
        }
        self.inner.read(buf)
    }
}

/// Answers one request. `client_slot` takes the place of the client a trusted
/// proxy names (`None` inside: the connection's own place is enough), or
/// says it has too many open.
fn handle(
    logger: &slog::Logger,
    mut stream: TcpStream,
    takes_body: fn(&str) -> bool,
    client_slot: impl FnOnce(Option<std::net::IpAddr>) -> Option<Option<Slot>>,
    handler: &dyn Fn(&Request) -> Response,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut rdr = std::io::BufReader::new(Deadline {
        inner: &stream,
        until: std::time::Instant::now() + REQUEST_DEADLINE,
    });
    let mut line = String::new();
    (&mut rdr).take(MAX_REQUEST_LINE).read_line(&mut line)?;
    debug!(logger, "Request: {}", line);
    let mut req = Request::parse_line(&line);
    req.peer = stream.peer_addr().ok().map(|a| a.ip());
    req.conn_peer = req.peer;
    // GETs are answered right after the request line, as before (the game
    // doesn't need its headers read). A POST's headers and body are read, if
    // it's to a path that takes one.
    if req.method == "POST" && !takes_body(req.path.split('?').next().unwrap_or_default()) {
        return stream.write_all(&Response::status("405 Method Not Allowed").to_bytes());
    }
    if req.method == "POST" {
        let mut length = 0usize;
        for _ in 0..64 {
            let mut header = String::new();
            (&mut rdr).take(MAX_REQUEST_LINE).read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(usize::MAX);
                }
                if name.trim().eq_ignore_ascii_case("x-forwarded-for") {
                    req.peer = crate::rate_limit::client_ip(req.peer, Some(value.trim()));
                }
                if name.trim().eq_ignore_ascii_case("x-forwarded-proto") {
                    req.forwarded_proto = Some(value.trim().chars().take(16).collect());
                }
            }
        }
        if length > MAX_BODY {
            return stream.write_all(&Response::status("413 Payload Too Large").to_bytes());
        }
        // Through a proxy, the client it names now has a place of its own, so one
        // client's slow bodies can't use up everyone's.
        let Some(_client_slot) = client_slot(req.peer) else {
            debug!(logger, "simple_http: too many connections from {:?}; refusing one", req.peer);
            return stream.write_all(&Response::status("429 Too Many Requests").to_bytes());
        };
        let until = std::time::Instant::now() + BODY_DEADLINE;
        rdr.get_mut().until = rdr.get_ref().until.min(until);
        stream.set_read_timeout(Some(BODY_DEADLINE))?;
        req.body = vec![0; length];
        rdr.read_exact(&mut req.body)?;
        let resp = handler(&req);
        debug!(logger, "Status {}", resp.status);
        return stream.write_all(&resp.to_bytes());
    }
    let resp = handler(&req);
    debug!(logger, "Status {}", resp.status);
    stream.write_all(&resp.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: SocketAddr, request: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn free_addr() -> SocketAddr {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
    }

    fn logger() -> slog::Logger {
        slog::Logger::root(slog::Discard, slog::o!())
    }

    #[test]
    fn an_idle_connection_does_not_block_others() {
        let addr = free_addr();
        std::thread::spawn(move || serve(&logger(), addr, "config", None, |_| false));
        std::thread::sleep(Duration::from_millis(100));
        // Connects and never sends a request.
        let _idle = TcpStream::connect(addr).unwrap();
        let resp = get(addr, "GET /OnlineConfigService.svc/GetOnlineConfig HTTP/1.1\r\n");
        assert!(resp.starts_with("HTTP/1.0 200 OK"), "{resp}");
        assert!(resp.ends_with("config"));
    }

    #[test]
    fn content_server_routes() {
        let dir = std::env::temp_dir().join(format!("fe-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mp.ini"), b"[mp]").unwrap();
        let files = HashMap::from([("/mp_balancing.ini".to_string(), dir.join("mp.ini")), ("/gone.ini".to_string(), dir.join("missing.ini"))]);
        let addr = free_addr();
        std::thread::spawn(move || serve_many(&logger(), addr, &files));
        std::thread::sleep(Duration::from_millis(100));

        assert!(get(addr, "GET /mp_balancing.ini HTTP/1.1\r\n").ends_with("[mp]"));
        assert!(get(addr, "GET /nope HTTP/1.1\r\n").starts_with("HTTP/1.0 404"));
        assert!(get(addr, "POST /mp_balancing.ini HTTP/1.1\r\nContent-Length: 0\r\n\r\n").starts_with("HTTP/1.0 405"));
        // A file that can't be read is an error response, not a dead server.
        assert!(get(addr, "GET /gone.ini HTTP/1.1\r\n").starts_with("HTTP/1.0 500"));
        assert!(get(addr, "GET /mp_balancing.ini HTTP/1.1\r\n").ends_with("[mp]"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn post_bodies_reach_the_routes_and_everything_else_is_the_config() {
        let addr = free_addr();
        let routes: Routes = Arc::new(|req: &Request| (req.method == "POST" && req.path == "/echo").then(|| Response::ok(req.body.clone())));
        std::thread::spawn(move || serve(&logger(), addr, "config", Some(routes), |path| path == "/echo"));
        std::thread::sleep(Duration::from_millis(100));
        let resp = get(addr, "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello");
        assert!(resp.ends_with("hello"), "{resp}");
        assert!(get(addr, "GET /anything HTTP/1.1\r\n").ends_with("config"));
        let too_big = format!("POST /echo HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(get(addr, &too_big).starts_with("HTTP/1.0 413"));
    }

    #[test]
    fn slow_posts_through_a_proxy_count_per_client() {
        let addr = free_addr();
        let routes: Routes = Arc::new(|req: &Request| (req.method == "POST" && req.path == "/echo").then(|| Response::ok(req.body.clone())));
        std::thread::spawn(move || serve(&logger(), addr, "config", Some(routes), |path| path == "/echo"));
        std::thread::sleep(Duration::from_millis(100));
        // One client, through a proxy on this machine, holding bodies open.
        let slow = |client: &str| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(format!("POST /echo HTTP/1.1\r\nX-Forwarded-For: {client}\r\nContent-Length: 5\r\n\r\nhe").as_bytes())
                .unwrap();
            s
        };
        let _held: Vec<TcpStream> = (0..MAX_OPEN_PER_IP).map(|_| slow("198.51.100.1")).collect();
        std::thread::sleep(Duration::from_millis(200));
        // Its next one is refused; everyone else, through the same proxy, still gets in.
        let over = get(addr, "POST /echo HTTP/1.1\r\nX-Forwarded-For: 198.51.100.1\r\nContent-Length: 2\r\n\r\nhi");
        assert!(over.starts_with("HTTP/1.0 429"), "{over}");
        let other = get(addr, "POST /echo HTTP/1.1\r\nX-Forwarded-For: 198.51.100.2\r\nContent-Length: 5\r\n\r\nhello");
        assert!(other.ends_with("hello"), "{other}");
        assert!(get(addr, "GET /OnlineConfigService.svc/GetOnlineConfig HTTP/1.1\r\n").ends_with("config"));
        // A POST nothing here takes is refused before its body.
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.write_all(b"POST /OnlineConfigService.svc/GetOnlineConfig HTTP/1.1\r\nContent-Length: 9000\r\n\r\n")
            .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.0 405"), "{out}");
    }
}
