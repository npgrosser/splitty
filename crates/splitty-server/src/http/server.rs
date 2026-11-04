//! The explicit browser listener admits anonymous sockets before allocating a
//! Hyper connection. Its reservation follows the actual IO through upgrades.
use axum::{body::Body, Router};
use futures::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use splitty_transport::{
    admission::{accept_backoff, Admission, Carrier},
    budget::{Permit, ProcessBudget},
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::watch,
    task::JoinSet,
};
use tower::ServiceExt;

#[cfg(test)]
use tokio::net::TcpListener;

enum Listener {
    Registered(splitty_remote::tunnel::exposure::GatewayListener),
    #[cfg(test)]
    Test(TcpListener),
}
impl Listener {
    async fn accept(&self) -> io::Result<(TcpStream, std::net::SocketAddr)> {
        match self {
            Self::Registered(listener) => listener.accept().await,
            #[cfg(test)]
            Self::Test(listener) => listener.accept().await,
        }
    }
}
#[cfg(test)]
impl From<TcpListener> for Listener {
    fn from(listener: TcpListener) -> Self {
        Self::Test(listener)
    }
}

const HTTP_BUFFER: usize = 64 * 1024;
// Read storage, parsed header ownership, write headers and capacity rounding.
// Response payload owners carry their own delivery leases; vectored writes
// below retain those owners instead of flattening a body into another Vec.
const HTTP_RESERVATION: usize = 4 * HTTP_BUFFER;
const HEADER_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = splitty_transport::wire::LIVENESS_TIMEOUT;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

type Stopped = Shared<BoxFuture<'static, ()>>;
struct Resources {
    _carrier: Carrier,
    _bytes: Permit,
    // Last field: notify completion only after the actual resource permits
    // have dropped, including aliases held by upgraded application tasks.
    _done: ResourceDone,
}
struct ResourceDone(watch::Sender<usize>);
impl Drop for ResourceDone {
    fn drop(&mut self) {
        self.0.send_modify(|active| *active -= 1);
    }
}
#[derive(Clone)]
pub(crate) struct Admitted {
    _resources: Arc<Resources>,
}

struct AdmittedIo {
    io: TcpStream,
    _resources: Admitted,
    read_stop: Stopped,
    write_stop: Stopped,
    write_timeout: Duration,
    stalled_write: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl AdmittedIo {
    fn write_deadline(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if self
            .stalled_write
            .as_mut()
            .is_some_and(|timer| timer.as_mut().poll(cx).is_ready())
        {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "browser response write stalled",
            ))
        } else {
            Ok(())
        }
    }
    fn writing<T>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match result {
            Poll::Pending => {
                if self.stalled_write.is_none() {
                    self.stalled_write = Some(Box::pin(tokio::time::sleep(self.write_timeout)));
                }
                self.write_deadline(cx)?;
                Poll::Pending
            }
            ready => {
                self.stalled_write = None;
                ready
            }
        }
    }

    fn stopped(stop: &mut Stopped, cx: &mut Context<'_>) -> io::Result<()> {
        if Pin::new(stop).poll(cx).is_ready() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "browser gateway stopped",
            ))
        } else {
            Ok(())
        }
    }
}
impl AsyncRead for AdmittedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Self::stopped(&mut self.read_stop, cx)?;
        Pin::new(&mut self.io).poll_read(cx, buffer)
    }
}
impl AsyncWrite for AdmittedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Self::stopped(&mut self.write_stop, cx)?;
        self.write_deadline(cx)?;
        let result = Pin::new(&mut self.io).poll_write(cx, bytes);
        self.writing(cx, result)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Self::stopped(&mut self.write_stop, cx)?;
        self.write_deadline(cx)?;
        let result = Pin::new(&mut self.io).poll_write_vectored(cx, bytes);
        self.writing(cx, result)
    }
    fn is_write_vectored(&self) -> bool {
        true
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Self::stopped(&mut self.write_stop, cx)?;
        self.write_deadline(cx)?;
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// The reservation and actual listener remain one owner for the whole server.
pub async fn serve_registered(
    listener: splitty_remote::tunnel::exposure::GatewayListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    serve_with(
        Listener::Registered(listener),
        app,
        shutdown,
        Admission::global(),
        ProcessBudget::global(),
        HEADER_TIMEOUT,
        WRITE_TIMEOUT,
    )
    .await
}

// In-process fixtures inject isolated listeners and admission pools. There is
// no production conversion from an arbitrary listener into a registered one.
#[cfg(test)]
pub(crate) async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    serve_with(
        listener,
        app,
        shutdown,
        Admission::global(),
        ProcessBudget::global(),
        HEADER_TIMEOUT,
        WRITE_TIMEOUT,
    )
    .await
}

async fn serve_with(
    listener: impl Into<Listener>,
    app: Router,
    shutdown: impl Future<Output = ()>,
    admission: Arc<Admission>,
    budget: Arc<ProcessBudget>,
    header_timeout: Duration,
    write_timeout: Duration,
) -> io::Result<()> {
    let listener = listener.into();
    let (stop, mut stopped) = watch::channel(false);
    let closed = async move {
        let _ = stopped.wait_for(|closed| *closed).await;
    }
    .boxed()
    .shared();
    let mut tasks = JoinSet::new();
    let (active, mut drained) = watch::channel(0usize);
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! { biased;
            _ = &mut shutdown => break Ok(()),
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let (io, _) = match accepted {
                    Ok(io) => io,
                    Err(error) => {
                        // A failed accept is not a failure of every admitted
                        // connection. Back off resource exhaustion as Axum did.
                        if let Some(delay) = accept_backoff(&error) {
                            tracing::warn!(%error, "browser accept failed; retrying");
                            tokio::select! { biased;
                                _ = &mut shutdown => break Ok(()),
                                _ = tokio::time::sleep(delay) => {},
                            }
                        }
                        continue;
                    }
                };
                let Ok(carrier) = admission.carrier() else { continue; };
                let Ok(bytes) = budget.data.acquire(HTTP_RESERVATION) else { continue; };
                active.send_modify(|count| *count += 1);
                let resources = Admitted { _resources: Arc::new(Resources {
                    _carrier: carrier, _bytes: bytes, _done: ResourceDone(active.clone()),
                }) };
                let io = AdmittedIo { io, _resources: resources.clone(), read_stop: closed.clone(), write_stop: closed.clone(), write_timeout, stalled_write: None };
                let app = app.clone(); let closing = closed.clone();
                tasks.spawn(async move {
                    let service = service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
                        request.extensions_mut().insert(resources.clone());
                        app.clone().oneshot(request.map(Body::new))
                    });
                    let mut builder = http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(header_timeout)
                        .max_headers(64).max_buf_size(HTTP_BUFFER).writev(true);
                    let connection = builder.serve_connection(TokioIo::new(io), service).with_upgrades();
                    tokio::select! { biased;
                        _ = closing => {},
                        result = connection => if let Err(error) = result { tracing::debug!(%error, "browser HTTP connection ended"); },
                    }
                });
            }
        }
    };
    let _ = stop.send(true);
    // Hyper finishes on upgrade, while the application task keeps the same
    // Admitted owner. Await both lifetimes under one shutdown deadline.
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    if tokio::time::timeout_at(deadline, async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    if tokio::time::timeout_at(deadline, drained.wait_for(|active| *active == 0))
        .await
        .is_err()
    {
        return result.and(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "browser connection cleanup did not finish",
        )));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const WAIT: Duration = Duration::from_secs(5);

    async fn until(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(WAIT, async {
            while !ready() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn start(
        admission: Arc<Admission>,
        budget: Arc<ProcessBudget>,
        header_timeout: Duration,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve_with(
            listener,
            Router::new().route("/health", axum::routing::get(|| async { "ok" })),
            async {
                let _ = stopped.await;
            },
            admission,
            budget,
            header_timeout,
            WRITE_TIMEOUT,
        ));
        (address, stop, task)
    }
    fn process() -> Arc<ProcessBudget> {
        ProcessBudget::new(128 * 1024 * 1024, 8 * 1024 * 1024)
    }

    #[test]
    fn failed_accepts_preserve_the_listener_and_back_off_resource_errors() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::Interrupted,
        ] {
            assert_eq!(accept_backoff(&io::Error::from(kind)), None);
        }
        assert_eq!(
            accept_backoff(&io::Error::from(io::ErrorKind::OutOfMemory)),
            Some(Duration::from_secs(1))
        );
    }

    #[tokio::test]
    async fn anonymous_socket_flood_reserves_before_tasks_and_recovers_after_disconnect() {
        let admission = Arc::new(Admission::default());
        let budget = process();
        let (address, stop, task) = start(admission.clone(), budget.clone(), HEADER_TIMEOUT).await;
        let mut sockets = Vec::new();
        for _ in 0..splitty_transport::admission::CARRIERS {
            sockets.push(TcpStream::connect(address).await.unwrap());
        }
        until(|| admission.counts().carriers == splitty_transport::admission::CARRIERS).await;
        assert_eq!(
            budget.used(),
            splitty_transport::admission::CARRIERS * HTTP_RESERVATION
        );
        let mut excess = TcpStream::connect(address).await.unwrap();
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(WAIT, excess.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(admission.counts().streams, 0);
        assert_eq!(admission.counts().opening, 0);
        assert_eq!(admission.counts().workers, 0);
        sockets.pop();
        until(|| admission.counts().carriers == splitty_transport::admission::CARRIERS - 1).await;
        let mut admitted = TcpStream::connect(address).await.unwrap();
        admitted
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(WAIT, admitted.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(admission.counts().carriers, 0);
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn partial_and_idle_http_headers_expire_without_upstream_work() {
        let admission = Arc::new(Admission::default());
        let budget = process();
        let (address, stop, task) = start(
            admission.clone(),
            budget.clone(),
            Duration::from_millis(100),
        )
        .await;
        let mut idle = TcpStream::connect(address).await.unwrap();
        let mut partial = TcpStream::connect(address).await.unwrap();
        partial
            .write_all(b"GET /health HTTP/1.1\r\nHost:")
            .await
            .unwrap();
        until(|| admission.counts().carriers == 2).await;
        let started = tokio::time::Instant::now();
        let mut response = Vec::new();
        tokio::time::timeout(WAIT, partial.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(response.is_empty() || response.starts_with(b"HTTP/1.1 408"));
        response.clear();
        tokio::time::timeout(WAIT, idle.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        until(|| admission.counts().carriers == 0).await;
        assert_eq!(budget.used(), 0);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unread_http_response_expires_and_releases_its_body_and_carrier() {
        struct Payload {
            bytes: Vec<u8>,
            _permit: Permit,
        }
        impl AsRef<[u8]> for Payload {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        let admission = Arc::new(Admission::default());
        let budget = process();
        let response_budget = budget.clone();
        let app = Router::new()
            .route(
                "/large",
                axum::routing::get(move || {
                    let budget = response_budget.clone();
                    async move {
                        let size = 8 * 1024 * 1024;
                        let permit = budget.data.acquire(size).unwrap();
                        Body::from(axum::body::Bytes::from_owner(Payload {
                            bytes: vec![b'x'; size],
                            _permit: permit,
                        }))
                    }
                }),
            )
            .route("/health", axum::routing::get(|| async { "ok" }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve_with(
            listener,
            app,
            async {
                let _ = stopped.await;
            },
            admission.clone(),
            budget.clone(),
            HEADER_TIMEOUT,
            Duration::from_millis(150),
        ));
        let mut unread = TcpStream::connect(address).await.unwrap();
        unread
            .write_all(b"GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        until(|| budget.used() >= HTTP_RESERVATION + 8 * 1024 * 1024).await;
        // Another browser stays usable while the first never reads its body.
        let mut healthy = TcpStream::connect(address).await.unwrap();
        healthy
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(WAIT, healthy.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        until(|| admission.counts().carriers == 0 && budget.used() == 0).await;
        drop(unread);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn upgraded_io_retains_its_carrier_and_parser_lease_until_revoked() {
        let admission = Arc::new(Admission::default());
        let budget = process();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (ended, mut ending) = tokio::sync::mpsc::channel(1);
        let app = Router::new().route(
            "/upgrade",
            axum::routing::get(move |mut request: axum::extract::Request| {
                let ended = ended.clone();
                async move {
                    assert!(request.extensions().get::<Admitted>().is_some());
                    let upgrade = hyper::upgrade::on(&mut request);
                    tokio::spawn(async move {
                        let mut io = TokioIo::new(upgrade.await.unwrap());
                        let mut data = [0; 3];
                        io.read_exact(&mut data).await.unwrap();
                        assert_eq!(&data, b"abc");
                        assert!(
                            io.read(&mut [0]).await.is_err(),
                            "shutdown revokes an already upgraded socket"
                        );
                        drop(io);
                        ended.send(()).await.unwrap();
                    });
                    axum::http::Response::builder()
                        .status(101)
                        .header("connection", "upgrade")
                        .header("upgrade", "test")
                        .body(Body::empty())
                        .unwrap()
                }
            }),
        );
        let task = tokio::spawn(serve_with(
            listener,
            app,
            async {
                let _ = stopped.await;
            },
            admission.clone(),
            budget.clone(),
            HEADER_TIMEOUT,
            WRITE_TIMEOUT,
        ));
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(b"GET /upgrade HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: test\r\n\r\nabc").await.unwrap();
        let mut headers = Vec::new();
        tokio::time::timeout(WAIT, async {
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await.unwrap());
            }
        })
        .await
        .unwrap();
        assert!(headers.starts_with(b"HTTP/1.1 101"));
        assert_eq!(admission.counts().carriers, 1);
        assert_eq!(budget.used(), HTTP_RESERVATION);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        tokio::time::timeout(WAIT, ending.recv())
            .await
            .unwrap()
            .unwrap();
        until(|| admission.counts().carriers == 0).await;
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn a_full_process_byte_budget_refuses_before_allocating_a_connection() {
        let admission = Arc::new(Admission::default());
        let budget = ProcessBudget::new(HTTP_RESERVATION, 0);
        let (address, stop, task) = start(admission.clone(), budget.clone(), HEADER_TIMEOUT).await;
        let _held = TcpStream::connect(address).await.unwrap();
        until(|| budget.used() == HTTP_RESERVATION).await;
        let mut excess = TcpStream::connect(address).await.unwrap();
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(WAIT, excess.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(admission.counts().carriers, 1);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(budget.used(), 0);
    }
}
