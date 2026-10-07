//! Bounds on what strangers can make the coordinator hold: connections (in all and per
//! address, closed when idle), and request bodies being read (a budget across every
//! connection, so many streams on many connections can't each buffer their route's limit).

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::ReadBuf;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// A connection that sends nothing for this long is closed (HTTP keep-alive clients open a
/// new one; HTTP/2 ones too).
const IDLE: Duration = Duration::from_secs(90);
/// How long a request may take, reading its body included (a report's 8 MB on a slow line).
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(60);
/// How long a request waits for room in the body budget before it's refused as busy.
const BUDGET_WAIT: Duration = Duration::from_secs(10);

/// A listener that refuses connections past `max` at once, or `max_per_ip` from one address
/// (IPv6 by /64; a proxy on this machine isn't counted per address, since everything through
/// it comes from its address), and closes connections idle for [`IDLE`].
pub struct Limited {
    listener: TcpListener,
    open: Arc<Mutex<Open>>,
    max: usize,
    max_per_ip: usize,
}

#[derive(Default)]
struct Open {
    all: usize,
    per_ip: HashMap<IpAddr, usize>,
}

impl Limited {
    pub fn new(listener: TcpListener, max: usize, max_per_ip: usize) -> Self {
        Self {
            listener,
            open: Arc::default(),
            max,
            max_per_ip,
        }
    }

    /// A place for a connection from `peer`, or none.
    fn take(&self, peer: SocketAddr) -> Option<Slot> {
        let ip = peer.ip().to_canonical();
        let ip = (!ip.is_loopback()).then(|| bucket(ip));
        let mut open = self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mine = ip.map_or(0, |ip| open.per_ip.get(&ip).copied().unwrap_or(0));
        if open.all >= self.max || mine >= self.max_per_ip {
            return None;
        }
        open.all += 1;
        if let Some(ip) = ip {
            *open.per_ip.entry(ip).or_default() += 1;
        }
        Some(Slot { open: Arc::clone(&self.open), ip })
    }
}

/// An address's bucket: IPv6 by /64 (one customer's), IPv4 as it is.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & (u128::MAX << 64)).into()),
        v4 => v4,
    }
}

impl axum::serve::Listener for Limited {
    type Io = Connection;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    // E.g. out of file descriptors: back off instead of stopping.
                    tracing::warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let Some(slot) = self.take(peer) else {
                tracing::debug!("too many connections; refusing {peer}'s");
                continue;
            };
            let _ = stream.set_nodelay(true);
            let connection = Connection {
                inner: stream,
                idle: Box::pin(tokio::time::sleep(IDLE)),
                _slot: slot,
            };
            return (connection, peer);
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// A connection's place, given back when it closes.
struct Slot {
    open: Arc<Mutex<Open>>,
    ip: Option<IpAddr>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        open.all = open.all.saturating_sub(1);
        if let Some(ip) = self.ip {
            if let Some(n) = open.per_ip.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    open.per_ip.remove(&ip);
                }
            }
        }
    }
}

/// A connection that fails once its client has sent nothing for [`IDLE`].
pub struct Connection {
    inner: TcpStream,
    idle: Pin<Box<tokio::time::Sleep>>,
    _slot: Slot,
}

impl AsyncRead for Connection {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    let until = tokio::time::Instant::now() + IDLE;
                    self.idle.as_mut().reset(until);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Pending => match self.idle.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "idle connection"))),
                Poll::Pending => Poll::Pending,
            },
            other => other,
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[std::io::IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// Requests with a body in progress per address (IPv6 by /64), so one client can't take a
/// whole [`BodyBudget`] with slow uploads while everyone else waits.
pub struct InFlight {
    per_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
    max: usize,
}

/// A request's place among its address's [`InFlight`], given back when it's done.
pub struct InFlightSlot {
    per_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl InFlight {
    pub fn new(max: usize) -> Self {
        Self { per_ip: Arc::default(), max }
    }

    /// A place for one more request from `ip`, or none (it has `max` in progress).
    pub fn take(&self, ip: IpAddr) -> Option<InFlightSlot> {
        let ip = bucket(ip.to_canonical());
        let mut per_ip = self.per_ip.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = per_ip.entry(ip).or_default();
        if *n >= self.max {
            return None;
        }
        *n += 1;
        Some(InFlightSlot {
            per_ip: Arc::clone(&self.per_ip),
            ip,
        })
    }
}

impl Drop for InFlightSlot {
    fn drop(&mut self) {
        let mut per_ip = self.per_ip.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = per_ip.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                per_ip.remove(&self.ip);
            }
        }
    }
}

/// Too many requests in progress from one address.
pub fn too_many() -> Response {
    (StatusCode::TOO_MANY_REQUESTS, "too many requests at once; try again in a moment").into_response()
}

/// The coordinator's request body budgets: member servers' (their secret checked before the
/// body is read), so no stranger's slow uploads hold up their pulses and reports; players'
/// support messages (up to 6 MB), one in progress per address, so they don't crowd out
/// everyone else's small requests (a few per address); and the admin UI's.
pub struct Budgets {
    pub member: BodyBudget,
    pub support: BodyBudget,
    pub support_in_flight: InFlight,
    pub public: BodyBudget,
    pub public_in_flight: InFlight,
    pub admin: BodyBudget,
    pub admin_in_flight: InFlight,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            member: BodyBudget::new(64 * 1024 * 1024),
            support: BodyBudget::new(36 * 1024 * 1024),
            support_in_flight: InFlight::new(1),
            public: BodyBudget::new(16 * 1024 * 1024),
            public_in_flight: InFlight::new(4),
            admin: BodyBudget::new(8 * 1024 * 1024),
            admin_in_flight: InFlight::new(8),
        }
    }
}

/// Bytes of request bodies that may be read at once, across every connection (in KiB).
pub struct BodyBudget(Arc<tokio::sync::Semaphore>);

impl BodyBudget {
    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(bytes / 1024)))
    }

    /// Runs `request` with room for a body of up to `limit` bytes reserved, within
    /// [`REQUEST_DEADLINE`]; busy (503) when there's no room within [`BUDGET_WAIT`].
    pub async fn run<F>(&self, limit: usize, request: F) -> Response
    where
        F: Future<Output = Response>,
    {
        self.run_waiting(limit, BUDGET_WAIT, request).await
    }

    async fn run_waiting<F>(&self, limit: usize, wait: Duration, request: F) -> Response
    where
        F: Future<Output = Response>,
    {
        let kib = u32::try_from(limit.div_ceil(1024)).unwrap_or(u32::MAX);
        let Ok(Ok(_room)) = tokio::time::timeout(wait, Arc::clone(&self.0).acquire_many_owned(kib)).await else {
            return (StatusCode::SERVICE_UNAVAILABLE, "busy; try again in a moment").into_response();
        };
        match tokio::time::timeout(REQUEST_DEADLINE, request).await {
            Ok(response) => response,
            Err(_) => (StatusCode::REQUEST_TIMEOUT, "the request took too long").into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connections_are_capped_in_all_and_per_address() {
        let limited = Limited::new(TcpListener::bind("127.0.0.1:0").await.unwrap(), 3, 2);
        let a: SocketAddr = "203.0.113.1:1000".parse().unwrap();
        let b: SocketAddr = "203.0.113.2:1000".parse().unwrap();
        let a1 = limited.take(a).unwrap();
        let _a2 = limited.take(a).unwrap();
        assert!(limited.take(a).is_none(), "a third from one address");
        let _b1 = limited.take(b).unwrap();
        assert!(limited.take(b).is_none(), "past the total");
        drop(a1);
        assert!(limited.take(b).is_some(), "a place given back");
        // IPv6 by /64.
        let limited = Limited::new(TcpListener::bind("127.0.0.1:0").await.unwrap(), 10, 1);
        let _c = limited.take("[2001:db8::1]:1".parse().unwrap()).unwrap();
        assert!(limited.take("[2001:db8::2]:1".parse().unwrap()).is_none());
        // A proxy on this machine isn't counted per address.
        let _p1 = limited.take("127.0.0.1:1".parse().unwrap()).unwrap();
        assert!(limited.take("127.0.0.1:2".parse().unwrap()).is_some());
    }

    #[test]
    fn requests_in_progress_are_capped_per_address() {
        let in_flight = InFlight::new(2);
        let a: IpAddr = "203.0.113.1".parse().unwrap();
        let first = in_flight.take(a).unwrap();
        let _second = in_flight.take(a).unwrap();
        assert!(in_flight.take(a).is_none());
        assert!(in_flight.take("203.0.113.2".parse().unwrap()).is_some(), "another address");
        assert!(in_flight.take("::ffff:203.0.113.1".parse().unwrap()).is_none(), "the same address written as IPv6");
        drop(first);
        assert!(in_flight.take(a).is_some());
    }

    #[tokio::test]
    async fn the_body_budget_refuses_when_full() {
        let budget = BodyBudget::new(8 * 1024);
        let held = Arc::clone(&budget.0).acquire_many_owned(8).await.unwrap();
        let busy = budget.run_waiting(1024, Duration::from_millis(50), async { StatusCode::OK.into_response() }).await;
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        drop(held);
        let ok = budget.run(8 * 1024, async { StatusCode::OK.into_response() }).await;
        assert_eq!(ok.status(), StatusCode::OK);
    }
}
