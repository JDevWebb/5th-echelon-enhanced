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
    pub peer: Option<std::net::IpAddr>,
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
/// except what `routes` answers (the community API).
pub fn serve(logger: &slog::Logger, addr: SocketAddr, content: &str, routes: Option<Routes>) -> std::io::Result<()> {
    let content = content.as_bytes().to_vec();
    serve_listener(logger, TcpListener::bind(addr)?, move |req| {
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
    serve_listener(logger, TcpListener::bind(addr)?, move |req| {
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
    })
}

/// Accepts connections forever, answering each on its own thread, so one slow
/// or idle client can't hold up everyone else (upstream served one connection
/// at a time without timeouts).
fn serve_listener<H>(logger: &slog::Logger, listener: TcpListener, handler: H) -> std::io::Result<()>
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
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
        let logger = logger.clone();
        let handler = Arc::clone(&handler);
        std::thread::spawn(move || {
            if let Err(e) = handle(&logger, stream, &*handler) {
                debug!(logger, "simple_http: {e}");
            }
        });
    }
}

fn handle(logger: &slog::Logger, mut stream: TcpStream, handler: &dyn Fn(&Request) -> Response) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut rdr = std::io::BufReader::new(&stream);
    let mut line = String::new();
    (&mut rdr).take(MAX_REQUEST_LINE).read_line(&mut line)?;
    debug!(logger, "Request: {}", line);
    let mut req = Request::parse_line(&line);
    req.peer = stream.peer_addr().ok().map(|a| a.ip());
    // GETs are answered right after the request line, as before (the game
    // doesn't need its headers read). A POST's headers and body are read.
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
            }
        }
        if length > MAX_BODY {
            return stream.write_all(&Response::status("413 Payload Too Large").to_bytes());
        }
        req.body = vec![0; length];
        rdr.read_exact(&mut req.body)?;
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
        std::thread::spawn(move || serve(&logger(), addr, "config", None));
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
        std::thread::spawn(move || serve(&logger(), addr, "config", Some(routes)));
        std::thread::sleep(Duration::from_millis(100));
        let resp = get(addr, "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello");
        assert!(resp.ends_with("hello"), "{resp}");
        assert!(get(addr, "GET /anything HTTP/1.1\r\n").ends_with("config"));
        let too_big = format!("POST /echo HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(get(addr, &too_big).starts_with("HTTP/1.0 413"));
    }
}
