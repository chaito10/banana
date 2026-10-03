//! Transport: the only place that touches operating system sockets.
//!
//! Higher layers depend on capabilities, not on socket details, so the same
//! discovery, NAT traversal and session code runs against real UDP/TCP and
//! against the deterministic simulator in [`crate::sim`].

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};

use crate::error::{Error, Result};
use crate::obs::Level;
use crate::wire::MAX_DATAGRAM;

/// A boxed future returned by transport traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A bidirectional byte stream, TCP or anything else reliable.
pub trait AsyncStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> AsyncStream for T {}

/// Datagram delivery to a peer address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// Who the datagram came from, as observed locally.
    pub from: SocketAddr,
    /// Payload, at most [`MAX_DATAGRAM`] bytes.
    pub data: Vec<u8>,
}

/// Unreliable message transport.
pub trait DatagramTransport: Send + Sync + 'static {
    /// Sends one datagram.
    fn send_to<'a>(&'a self, target: SocketAddr, data: &'a [u8]) -> BoxFuture<'a, Result<()>>;

    /// Receives the next datagram.
    fn recv_from<'a>(&'a self) -> BoxFuture<'a, Result<Datagram>>;

    /// The address this transport is bound to.
    fn local_addr(&self) -> Result<SocketAddr>;

    /// Closes the transport and wakes up pending receive calls.
    fn close(&self) -> BoxFuture<'_, ()>;
}

/// Reliable, connection oriented transport.
pub trait StreamTransport: Send + Sync + 'static {
    /// Opens an outbound stream.
    fn connect<'a>(&'a self, target: SocketAddr) -> BoxFuture<'a, Result<Box<dyn AsyncStream>>>;

    /// Accepts the next inbound stream.
    fn accept<'a>(&'a self) -> BoxFuture<'a, Result<(SocketAddr, Box<dyn AsyncStream>)>>;

    /// The address this transport listens on, when it has one.
    fn local_addr(&self) -> Result<Option<SocketAddr>>;

    /// Stops accepting connections.
    fn close(&self) -> BoxFuture<'_, ()>;
}

/// Counters every transport keeps, so resource use stays observable.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    sent: AtomicU64,
    received: AtomicU64,
    dropped: AtomicU64,
    rejected: AtomicU64,
}

impl Counters {
    pub(crate) fn snapshot(&self) -> TransportStats {
        TransportStats {
            sent: self.sent.load(Ordering::Relaxed),
            received: self.received.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_received(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }
}

/// A snapshot of transport counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportStats {
    /// Datagrams handed to the operating system.
    pub sent: u64,
    /// Datagrams delivered to the application.
    pub received: u64,
    /// Datagrams discarded because the receive queue was full.
    pub dropped: u64,
    /// Datagrams rejected as oversized.
    pub rejected: u64,
}

/// A real UDP socket with a bounded receive queue.
pub struct UdpTransport {
    socket: Arc<UdpSocket>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Result<Datagram>>>,
    shutdown: watch::Sender<bool>,
    counters: Arc<Counters>,
}

impl UdpTransport {
    /// Binds a UDP socket and starts its receive task.
    pub async fn bind(address: SocketAddr, queue_depth: usize) -> Result<Self> {
        let bound = UdpSocket::bind(address)
            .await
            .map_err(|e| Error::io("bind udp", e))?;
        let socket = Arc::new(bound);
        let (tx, rx) = mpsc::channel(std::cmp::max(1, queue_depth));
        let (shutdown, mut closed) = watch::channel(false);
        let counters = Arc::new(Counters::default());

        let reader = socket.clone();
        let reader_counters = counters.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_DATAGRAM + 1];
            loop {
                let received = tokio::select! {
                    biased;
                    _ = closed.changed() => break,
                    result = reader.recv_from(&mut buf) => result,
                };
                let (len, from) = match received {
                    Ok(value) => value,
                    Err(_) => break,
                };
                if len > MAX_DATAGRAM {
                    reader_counters.record_rejected();
                    continue;
                }
                reader_counters.record_received();
                let datagram = Datagram {
                    from,
                    data: buf[..len].to_vec(),
                };
                if tx.send(Ok(datagram)).await.is_err() {
                    // The consumer is gone, so nothing received from here on
                    // can be delivered; that is a drop, not a rejection.
                    reader_counters.record_dropped();
                    break;
                }
            }
        });

        Ok(UdpTransport {
            socket,
            inbox: tokio::sync::Mutex::new(rx),
            shutdown,
            counters,
        })
    }

    /// Reads transport counters.
    pub fn stats(&self) -> TransportStats {
        self.counters.snapshot()
    }
}

impl DatagramTransport for UdpTransport {
    fn send_to<'a>(&'a self, target: SocketAddr, data: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if data.len() > MAX_DATAGRAM {
                self.counters.record_rejected();
                return Err(Error::Limit {
                    what: "datagram",
                    limit: MAX_DATAGRAM as u64,
                });
            }
            self.counters.record_sent();
            self.socket
                .send_to(data, target)
                .await
                .map(|_| ())
                .map_err(|e| Error::io("send udp", e))
        })
    }

    fn recv_from<'a>(&'a self) -> BoxFuture<'a, Result<Datagram>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            match inbox.recv().await {
                Some(Ok(datagram)) => Ok(datagram),
                Some(Err(e)) => Err(e),
                None => Err(Error::Closed {
                    what: "udp transport",
                }),
            }
        })
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        self.socket
            .local_addr()
            .map_err(|e| Error::io("udp local addr", e))
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.shutdown.send(true);
        })
    }
}

/// A real TCP listener with a bounded accept queue.
pub struct TcpTransport {
    listener: Arc<TcpListener>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Result<(SocketAddr, Box<dyn AsyncStream>)>>>,
    shutdown: watch::Sender<bool>,
    counters: Arc<Counters>,
}

impl TcpTransport {
    /// Binds a TCP listener and starts its accept task.
    pub async fn bind(address: SocketAddr, queue_depth: usize) -> Result<Self> {
        let bound = TcpListener::bind(address)
            .await
            .map_err(|e| Error::io("bind tcp", e))?;
        let listener = Arc::new(bound);
        let (tx, rx) = mpsc::channel(std::cmp::max(1, queue_depth));
        let (shutdown, mut closed) = watch::channel(false);
        let counters = Arc::new(Counters::default());
        let accept_counters = counters.clone();
        let accept_listener = listener.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = closed.changed() => break,
                    result = accept_listener.accept() => result,
                };
                match accepted {
                    Ok((stream, peer)) => {
                        let _ = stream.set_nodelay(true);
                        accept_counters.record_received();
                        let boxed: Box<dyn AsyncStream> = Box::new(stream);
                        if tx.send(Ok((peer, boxed))).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        crate::obs::log(Level::Debug, "tcp.accept", &[("error", e.to_string())]);
                    }
                }
            }
        });
        Ok(TcpTransport {
            listener,
            inbox: tokio::sync::Mutex::new(rx),
            shutdown,
            counters,
        })
    }

    /// Reads transport counters.
    pub fn stats(&self) -> TransportStats {
        self.counters.snapshot()
    }
}

impl StreamTransport for TcpTransport {
    fn connect<'a>(&'a self, target: SocketAddr) -> BoxFuture<'a, Result<Box<dyn AsyncStream>>> {
        Box::pin(async move {
            let stream = TcpStream::connect(target)
                .await
                .map_err(|e| Error::io("tcp connect", e))?;
            let _ = stream.set_nodelay(true);
            self.counters.record_sent();
            Ok(Box::new(stream) as Box<dyn AsyncStream>)
        })
    }

    fn accept<'a>(&'a self) -> BoxFuture<'a, Result<(SocketAddr, Box<dyn AsyncStream>)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            match inbox.recv().await {
                Some(Ok(value)) => Ok(value),
                Some(Err(e)) => Err(e),
                None => Err(Error::Closed {
                    what: "tcp transport",
                }),
            }
        })
    }

    fn local_addr(&self) -> Result<Option<SocketAddr>> {
        self.listener
            .local_addr()
            .map(Some)
            .map_err(|e| Error::io("tcp local addr", e))
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.shutdown.send(true);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn udp_round_trips() {
        let server = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 8)
            .await
            .unwrap();
        let client = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 8)
            .await
            .unwrap();
        let server_addr = server.local_addr().unwrap();
        client.send_to(server_addr, b"ping").await.unwrap();
        let datagram = server.recv_from().await.unwrap();
        assert_eq!(datagram.data, b"ping");
        assert_eq!(datagram.from, client.local_addr().unwrap());
        server.send_to(datagram.from, b"pong").await.unwrap();
        assert_eq!(client.recv_from().await.unwrap().data, b"pong");
        assert_eq!(client.stats().sent, 1);
        assert_eq!(server.stats().received, 1);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn udp_rejects_oversized_datagrams() {
        let client = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 8)
            .await
            .unwrap();
        let payload = vec![0u8; MAX_DATAGRAM + 1];
        let err = client
            .send_to("127.0.0.1:1".parse().unwrap(), &payload)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Limit {
                what: "datagram",
                ..
            }
        ));
        assert_eq!(client.stats().rejected, 1);
    }

    #[tokio::test]
    async fn tcp_streams_both_ways() {
        let listener = TcpTransport::bind("127.0.0.1:0".parse().unwrap(), 4)
            .await
            .unwrap();
        let bound = listener.local_addr().unwrap().unwrap();
        let dialer = TcpTransport::bind("127.0.0.1:0".parse().unwrap(), 4)
            .await
            .unwrap();
        let (outbound, inbound) = tokio::join!(dialer.connect(bound), listener.accept());
        let mut outbound = outbound.unwrap();
        let (peer, mut inbound) = inbound.unwrap();
        assert_ne!(peer.port(), 0);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        outbound.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        inbound.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        inbound.write_all(b"world").await.unwrap();
        outbound.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
        listener.close().await;
        dialer.close().await;
    }

    #[tokio::test]
    async fn closed_transport_reports_closed() {
        let transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4)
            .await
            .unwrap();
        transport.close().await;
        let err = transport.recv_from().await.unwrap_err();
        assert!(matches!(
            err,
            Error::Closed {
                what: "udp transport"
            }
        ));
    }
}
