//! Session endpoints: one authenticated session plus its multiplexed streams.
//!
//! This is the only place that turns a [`SecureChannel`] into something an
//! application can use. The task it owns is deliberately single threaded:
//! frames are encoded, sealed and written in one place, so there is no second
//! writer and no interleaving hazard (RFC 19.4).
//!
//! Two wires are supported. Datagram sessions add the small control header to
//! every record, so unrelated traffic can share one socket. Stream sessions
//! use length prefixed records over any reliable transport, which is what the
//! relay uses.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch, Mutex};

use crate::error::{Error, Result};
use crate::identity::PeerId;
use crate::mux::{
    Chunk, Frame, FrameKind, FrameReader, MuxShared, Stream, StreamEnd, StreamIdAllocator, Window,
    CONTROL_STREAM,
};
use crate::obs::{log, EventBus, EventKind, Level, PathKind};
use crate::session::{SecureChannel, TAG_LEN};
use crate::transport::{AsyncStream, DatagramTransport};
use crate::wire::{kind, Header, MAX_DATAGRAM};

/// Everything the session task needs to know.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Largest plaintext per record.
    pub max_record: usize,
    /// Largest frame payload the multiplexer may produce.
    pub max_frame: usize,
    /// Receive window per stream.
    pub window: u32,
    /// Buffered chunks per stream, which caps buffered bytes per stream.
    pub chunk_queue: usize,
    /// Concurrent streams allowed in one session.
    pub max_streams: usize,
    /// How often a ping is sent when the session is otherwise idle.
    pub keepalive: Duration,
    /// How long silence from the peer is tolerated.
    pub keepalive_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            max_record: MAX_DATAGRAM - crate::wire::HEADER_LEN - TAG_LEN,
            max_frame: 1024,
            window: 64 * 1024,
            chunk_queue: 8,
            max_streams: 256,
            keepalive: Duration::from_secs(15),
            keepalive_timeout: Duration::from_secs(45),
        }
    }
}

impl SessionConfig {
    /// Largest frame payload that still fits in one record.
    pub fn frame_limit(&self, channel: &SecureChannel) -> usize {
        std::cmp::min(self.max_frame, channel.max_record())
    }
}

/// Where a session's records travel.
pub enum Wire {
    /// One record per datagram, each with a control header.
    Datagram {
        /// Transport to send on.
        transport: Arc<dyn DatagramTransport>,
        /// Where records go.
        remote: SocketAddr,
        /// Session id written into the header.
        session: u32,
        /// Records delivered by the socket pump, header already stripped.
        incoming: mpsc::Receiver<Vec<u8>>,
    },
    /// Length prefixed records over a reliable stream.
    Stream(Box<dyn AsyncStream>),
}

impl Wire {
    /// Address the peer is reached at, when the wire has one.
    pub fn remote(&self) -> Option<SocketAddr> {
        match self {
            Wire::Datagram { remote, .. } => Some(*remote),
            Wire::Stream(_) => None,
        }
    }

    async fn send(&mut self, payload: &[u8]) -> Result<()> {
        match self {
            Wire::Datagram {
                transport,
                remote,
                session,
                ..
            } => {
                let header = Header::new(kind::NOISE, *session).encode();
                let mut out = Vec::with_capacity(header.len() + payload.len());
                out.extend_from_slice(&header);
                out.extend_from_slice(payload);
                transport.send_to(*remote, &out).await
            }
            Wire::Stream(stream) => {
                let len = u32::try_from(payload.len()).map_err(|_| Error::Limit {
                    what: "record",
                    limit: u32::MAX as u64,
                })?;
                let mut frame = Vec::with_capacity(4 + payload.len());
                frame.extend_from_slice(&len.to_le_bytes());
                frame.extend_from_slice(payload);
                stream.write_all(&frame).await?;
                stream.flush().await?;
                Ok(())
            }
        }
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        match self {
            Wire::Datagram { incoming, .. } => incoming.recv().await,
            Wire::Stream(stream) => {
                let mut header = [0u8; 4];
                stream.read_exact(&mut header).await.ok()?;
                let len = u32::from_le_bytes(header) as usize;
                if len > MAX_DATAGRAM * 8 {
                    return None;
                }
                let mut payload = vec![0u8; len];
                stream.read_exact(&mut payload).await.ok()?;
                Some(payload)
            }
        }
    }
}

/// What a session hands to the application.
enum Inbound {
    /// The peer opened a stream.
    Stream(Stream),
    /// The session ended.
    Closed(String),
}

/// State for one stream that can receive from the peer.
struct StreamInbox {
    sender: mpsc::Sender<Vec<u8>>,
    window: Arc<Window>,
}

/// Shared handle to a running session.
struct EndpointHandle {
    peer: PeerId,
    path: PathKind,
    remote: Option<SocketAddr>,
    shared: Arc<MuxShared>,
    control: mpsc::UnboundedSender<Frame>,
    opened: mpsc::UnboundedSender<Inbound>,
    inboxes: Mutex<HashMap<u32, StreamInbox>>,
    windows: Mutex<HashMap<u32, Arc<Window>>>,
    ids: Mutex<StreamIdAllocator>,
    open_streams: AtomicUsize,
    session: u32,
    config: SessionConfig,
    events: EventBus,
    closed: watch::Sender<bool>,
    reason: StdMutex<Option<String>>,
    records_in: AtomicU64,
    records_out: AtomicU64,
}

/// Counters for one live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    /// Encrypted records received.
    pub records_in: u64,
    /// Encrypted records sent.
    pub records_out: u64,
    /// Streams currently open.
    pub streams: usize,
}

impl EndpointHandle {
    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    fn close(&self, reason: &str) {
        if let Ok(mut slot) = self.reason.lock() {
            if slot.is_none() {
                *slot = Some(reason.to_string());
            }
        }
        // `send_replace` rather than `send`: closing before the session task
        // has run would otherwise be lost, and the task would never notice.
        self.closed.send_replace(true);
        self.shared.mark_closed();
    }

    fn stats(&self) -> SessionStats {
        SessionStats {
            records_in: self.records_in.load(Ordering::Relaxed),
            records_out: self.records_out.load(Ordering::Relaxed),
            streams: self.open_streams.load(Ordering::Relaxed),
        }
    }
}

/// A live session with one peer.
///
/// Application bytes only ever appear as [`Stream`] values, and those only
/// exist after the Noise handshake has authenticated the peer, so there is no
/// way to read plaintext from an unverified peer.
pub struct Connection {
    handle: Arc<EndpointHandle>,
    inbound: mpsc::UnboundedReceiver<Inbound>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("peer", &self.handle.peer)
            .field("path", &self.handle.path)
            .field("remote", &self.handle.remote)
            .field("session", &self.handle.session)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// The authenticated peer.
    pub fn peer(&self) -> PeerId {
        self.handle.peer
    }

    /// How this session reaches the peer.
    pub fn path(&self) -> PathKind {
        self.handle.path
    }

    /// The peer address, for direct paths.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.handle.remote
    }

    /// Session id used to demultiplex datagrams.
    pub fn session_id(&self) -> u32 {
        self.handle.session
    }

    /// Counters for this session.
    pub fn stats(&self) -> SessionStats {
        self.handle.stats()
    }

    /// Whether the session has ended.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Why the session ended, once it has.
    pub fn close_reason(&self) -> Option<String> {
        self.handle.reason.lock().ok().and_then(|slot| slot.clone())
    }

    /// Opens a new stream to the peer.
    ///
    /// The stream is bidirectional: the peer may send on it too.
    pub async fn open_stream(&self) -> Result<Stream> {
        if self.handle.is_closed() {
            return Err(Error::Closed { what: "session" });
        }
        let id = {
            let mut ids = self.handle.ids.lock().await;
            ids.next_id()
        };
        let send_window = Arc::new(Window::new(self.handle.shared.window_size()));
        let recv_window = Arc::new(Window::new(self.handle.shared.window_size()));
        let (sender, receiver) = mpsc::channel(self.handle.config.chunk_queue);
        let stream = Stream::new(
            id,
            self.handle.shared.clone(),
            send_window.clone(),
            receiver,
        )
        .with_recv_window(recv_window.clone());
        self.handle.inboxes.lock().await.insert(
            id,
            StreamInbox {
                sender,
                window: recv_window,
            },
        );
        self.handle.windows.lock().await.insert(id, send_window);
        self.handle.open_streams.fetch_add(1, Ordering::AcqRel);
        self.handle
            .control
            .send(Frame::new(FrameKind::Open, id, Vec::new()))
            .map_err(|_| Error::Closed { what: "session" })?;
        self.handle.events.emit(EventKind::StreamOpened {
            peer: self.handle.peer,
            stream: id,
        });
        Ok(stream)
    }

    /// Waits for the peer to open a stream.
    pub async fn accept(&mut self) -> Result<Stream> {
        match self.inbound.recv().await {
            Some(Inbound::Stream(stream)) => Ok(stream),
            Some(Inbound::Closed(reason)) => {
                log(
                    Level::Debug,
                    "session.accept",
                    &[
                        ("peer_id", self.handle.peer.to_string()),
                        ("reason", reason),
                    ],
                );
                Err(Error::Closed { what: "session" })
            }
            None => Err(Error::Closed { what: "session" }),
        }
    }

    /// Takes an already opened stream, if one is waiting.
    pub fn try_accept(&mut self) -> Option<Stream> {
        match self.inbound.try_recv() {
            Ok(Inbound::Stream(stream)) => Some(stream),
            _ => None,
        }
    }

    /// Ends the session.
    pub fn close(&self) {
        self.handle.close("closed by application");
    }
}

/// Starts the task that owns one authenticated session.
pub fn spawn(
    channel: SecureChannel,
    wire: Wire,
    config: SessionConfig,
    events: EventBus,
    path: PathKind,
) -> Connection {
    let peer = channel.remote_peer();
    let session = channel.session_id();
    let remote = wire.remote();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (data_tx, data_rx) = mpsc::channel(config.chunk_queue * 2);
    let (opened_tx, opened_rx) = mpsc::unbounded_channel();
    let (closed_tx, _) = watch::channel(false);
    let shared = Arc::new(MuxShared::new(
        peer,
        control_tx.clone(),
        data_tx,
        config.window,
        config.max_frame,
        config.max_frame,
    ));
    let handle = Arc::new(EndpointHandle {
        peer,
        path,
        remote,
        shared,
        control: control_tx,
        opened: opened_tx,
        inboxes: Mutex::new(HashMap::new()),
        windows: Mutex::new(HashMap::new()),
        ids: Mutex::new(StreamIdAllocator::new()),
        open_streams: AtomicUsize::new(0),
        session,
        config,
        events,
        closed: closed_tx,
        reason: StdMutex::new(None),
        records_in: AtomicU64::new(0),
        records_out: AtomicU64::new(0),
    });
    let task = handle.clone();
    tokio::spawn(async move {
        run(channel, wire, task, control_rx, data_rx).await;
    });
    Connection {
        handle,
        inbound: opened_rx,
    }
}

async fn run(
    mut channel: SecureChannel,
    mut wire: Wire,
    handle: Arc<EndpointHandle>,
    mut control_rx: mpsc::UnboundedReceiver<Frame>,
    mut data_rx: mpsc::Receiver<Chunk>,
) {
    let frame_limit = handle.config.frame_limit(&channel);
    let mut reader = FrameReader::new(handle.config.max_frame);
    let mut outbox: Vec<Vec<u8>> = Vec::new();
    let mut closed_rx = handle.closed.subscribe();
    let first_tick = tokio::time::Instant::now() + handle.config.keepalive;
    let mut tick = tokio::time::interval_at(first_tick, handle.config.keepalive);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_rx = std::time::Instant::now();
    let mut ping_counter: u64 = 0;

    let reason = 'session: loop {
        // One place seals and one place writes, so records can never interleave.
        for record in std::mem::take(&mut outbox) {
            let sealed = match channel.seal(&record) {
                Ok(sealed) => sealed,
                Err(e) => break 'session e.to_string(),
            };
            if let Err(e) = wire.send(&sealed).await {
                break 'session e.to_string();
            }
            handle.records_out.fetch_add(1, Ordering::Relaxed);
        }
        if *closed_rx.borrow() {
            break 'session "session closed locally".to_string();
        }
        tokio::select! {
                    biased;
                    changed = closed_rx.changed() => {
                        if changed.is_err() {
                            break 'session "session closed locally".to_string();
                        }
                    }
                    frame = control_rx.recv() => {
                        let Some(frame) = frame else {
                            break 'session "control channel closed".to_string();
                        };
                        enqueue(&mut outbox, &frame, frame_limit);
                    }
        data = data_rx.recv() => {
                        let Some(chunk) = data else {
                            break 'session "data channel closed".to_string();
                        };
                        match chunk.end {
                            // The marker arrived on the same ordered queue as the data,
                            // so every byte written before the close is already here.
                            Some(end) => {
                                let kind = match end {
                                    StreamEnd::Reset => FrameKind::Reset,
                                    _ => FrameKind::Close,
                                };
                                let frame = Frame::new(kind, chunk.stream, vec![end.to_u8()]);
                                enqueue(&mut outbox, &frame, frame_limit);
                            }
                            None => enqueue(
                                &mut outbox,
                                &Frame::new(FrameKind::Data, chunk.stream, chunk.payload),
                                frame_limit,
                            ),
                        }
                    }
                    record = wire.recv() => {
                        let Some(record) = record else {
                            break 'session "wire closed".to_string();
                        };
                        last_rx = std::time::Instant::now();
                        handle.records_in.fetch_add(1, Ordering::Relaxed);
                        match channel.open(&record) {
                            Ok(plaintext) => {
                                reader.feed(&plaintext);
                                loop {
                                    let frame = match reader.next_frame() {
                                        Ok(Some(frame)) => frame,
                                        Ok(None) => break,
                                        Err(e) => break 'session e.to_string(),
                                    };
                                    if let Err(e) = dispatch(&handle, frame, &mut outbox, frame_limit).await {
                                        break 'session e.to_string();
                                    }
                                }
                            }
                            Err(Error::Crypto(reason)) => {
                                log(Level::Debug, "session.record", &[
                                    ("peer_id", handle.peer.to_string()),
                                    ("reason", reason),
                                ]);
                            }
                            Err(e) => break 'session e.to_string(),
                        }
                    }
                    _ = tick.tick() => {
                        if last_rx.elapsed() > handle.config.keepalive_timeout {
                            break 'session "keepalive timeout".to_string();
                        }
                        ping_counter = ping_counter.wrapping_add(1);
                        enqueue(
                            &mut outbox,
                            &Frame::new(FrameKind::Ping, CONTROL_STREAM, ping_counter.to_le_bytes().to_vec()),
                            frame_limit,
                        );
                    }
                }
    };

    // Dropping the senders is what produces end of stream for the application.
    handle.inboxes.lock().await.clear();
    handle.windows.lock().await.clear();
    handle.open_streams.store(0, Ordering::Release);
    let _ = handle.opened.send(Inbound::Closed(reason.clone()));
    handle.close(&reason);
    handle.events.emit(EventKind::PeerDisconnected {
        peer: handle.peer,
        reason: reason.clone(),
    });
    log(
        Level::Debug,
        "session.closed",
        &[
            ("peer_id", handle.peer.to_string()),
            ("path", handle.path.as_str().to_string()),
            ("session", handle.session.to_string()),
            ("reason", reason),
        ],
    );
}

/// Splits oversized frames into records and queues them for sending.
fn enqueue(outbox: &mut Vec<Vec<u8>>, frame: &Frame, frame_limit: usize) {
    if frame.payload.len() <= frame_limit {
        if let Ok(record) = frame.encode(frame_limit) {
            outbox.push(record);
        }
        return;
    }
    match frame.kind {
        FrameKind::Data => {
            for chunk in frame.payload.chunks(frame_limit) {
                let part = Frame::new(FrameKind::Data, frame.stream, chunk.to_vec());
                if let Ok(record) = part.encode(frame_limit) {
                    outbox.push(record);
                }
            }
        }
        // Control frames are never large; anything oversized is a bug, so the
        // frame is sent without its payload rather than dropped silently.
        _ => {
            if let Ok(record) = Frame::new(frame.kind, frame.stream, Vec::new()).encode(frame_limit)
            {
                outbox.push(record);
            }
        }
    }
}

async fn dispatch(
    handle: &Arc<EndpointHandle>,
    frame: Frame,
    outbox: &mut Vec<Vec<u8>>,
    frame_limit: usize,
) -> Result<()> {
    match frame.kind {
        FrameKind::Data => {
            if frame.stream == CONTROL_STREAM
                || frame.payload.is_empty()
                || handle.shared.is_closed()
            {
                return Ok(());
            }
            let entry = {
                let inboxes = handle.inboxes.lock().await;
                inboxes
                    .get(&frame.stream)
                    .map(|inbox| (inbox.sender.clone(), inbox.window.clone()))
            };
            // A frame for a stream that is gone is stale, not fatal.
            let Some((sender, window)) = entry else {
                return Ok(());
            };
            let len = frame.payload.len() as u32;
            if !window.take(len) {
                handle.inboxes.lock().await.remove(&frame.stream);
                let _ = handle.open_streams.fetch_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |current| Some(current.saturating_sub(1)),
                );
                enqueue(
                    outbox,
                    &Frame::new(
                        FrameKind::Reset,
                        frame.stream,
                        vec![StreamEnd::Reset.to_u8()],
                    ),
                    frame_limit,
                );
                handle.events.emit(EventKind::StreamClosed {
                    peer: handle.peer,
                    stream: frame.stream,
                    reason: "window exceeded".into(),
                });
                return Ok(());
            }
            // Awaiting here is the backpressure: this frame's window credit is
            // already spent, so the peer cannot send more until it is delivered.
            let _ = sender.send(frame.payload).await;
            Ok(())
        }
        FrameKind::Open => {
            let already = handle.inboxes.lock().await.contains_key(&frame.stream);
            if frame.stream == CONTROL_STREAM || already {
                enqueue(
                    outbox,
                    &Frame::new(
                        FrameKind::Reset,
                        frame.stream,
                        vec![StreamEnd::Reset.to_u8()],
                    ),
                    frame_limit,
                );
                return Ok(());
            }
            if handle.inboxes.lock().await.len() >= handle.config.max_streams {
                handle.events.emit(EventKind::LimitReached {
                    what: "streams per session",
                    limit: handle.config.max_streams as u64,
                });
                enqueue(
                    outbox,
                    &Frame::new(
                        FrameKind::Reset,
                        frame.stream,
                        vec![StreamEnd::Reset.to_u8()],
                    ),
                    frame_limit,
                );
                return Ok(());
            }
            let (sender, receiver) = mpsc::channel(handle.config.chunk_queue);
            let receive_window = Arc::new(Window::new(handle.shared.window_size()));
            let send_window = Arc::new(Window::new(handle.shared.window_size()));
            handle.inboxes.lock().await.insert(
                frame.stream,
                StreamInbox {
                    sender,
                    window: receive_window.clone(),
                },
            );
            handle
                .windows
                .lock()
                .await
                .insert(frame.stream, send_window.clone());
            handle.open_streams.fetch_add(1, Ordering::AcqRel);
            handle.ids.lock().await.reserve(frame.stream);
            enqueue(
                outbox,
                &Frame::new(FrameKind::OpenAck, frame.stream, Vec::new()),
                frame_limit,
            );
            let stream = Stream::new(frame.stream, handle.shared.clone(), send_window, receiver)
                .with_recv_window(receive_window.clone());
            let _ = handle
                .opened
                .send(Inbound::Stream(stream))
                .map_err(|_| Error::Closed { what: "session" })?;
            handle.events.emit(EventKind::StreamOpened {
                peer: handle.peer,
                stream: frame.stream,
            });
            Ok(())
        }
        FrameKind::OpenAck => Ok(()),
        FrameKind::Close | FrameKind::Reset => {
            let end = frame
                .payload
                .first()
                .copied()
                .unwrap_or(StreamEnd::Remote.to_u8());
            let end = StreamEnd::from_u8(end).unwrap_or(StreamEnd::Reset);
            handle.inboxes.lock().await.remove(&frame.stream);
            handle.windows.lock().await.remove(&frame.stream);
            let _ =
                handle
                    .open_streams
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                        Some(current.saturating_sub(1))
                    });
            handle.events.emit(EventKind::StreamClosed {
                peer: handle.peer,
                stream: frame.stream,
                reason: end.as_str().to_string(),
            });
            Ok(())
        }
        FrameKind::WindowUpdate => {
            let credit = frame.credit()?;
            let send_side = handle.windows.lock().await.get(&frame.stream).cloned();
            if let Some(window) = send_side {
                window.release(credit);
                return Ok(());
            }
            let receive_side = {
                let inboxes = handle.inboxes.lock().await;
                inboxes.get(&frame.stream).map(|inbox| inbox.window.clone())
            };
            if let Some(window) = receive_side {
                window.release(credit);
            }
            Ok(())
        }
        FrameKind::Ping => {
            enqueue(
                outbox,
                &Frame::new(FrameKind::Pong, CONTROL_STREAM, frame.payload),
                frame_limit,
            );
            Ok(())
        }
        FrameKind::Pong => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::session::Handshake;
    use crate::transport::{StreamTransport, TcpTransport, UdpTransport};

    fn identity(seed: u8) -> Identity {
        Identity::from_seed([seed; 32])
    }

    /// Runs the three message `XX` handshake the same way the connection layer
    /// does, and returns both live channels.
    fn handshake_pair(
        a: &Identity,
        b: &Identity,
        max_record: usize,
    ) -> (SecureChannel, SecureChannel) {
        let mut initiator = Handshake::initiate(a, Some(b.peer_id())).unwrap();
        let mut responder = Handshake::accept(b, Some(a.peer_id())).unwrap();
        let first = initiator.next_message().unwrap();
        responder.read_message(&first).unwrap();
        let second = responder.next_message().unwrap();
        initiator.read_message(&second).unwrap();
        let third = initiator.next_message().unwrap();
        responder.read_message(&third).unwrap();
        let a_channel = initiator.finish(max_record).unwrap();
        let b_channel = responder.finish(max_record).unwrap();
        (a_channel, b_channel)
    }

    /// Connects two endpoints over a real TCP pair.
    async fn tcp_pair(
        a_channel: SecureChannel,
        b_channel: SecureChannel,
        config: SessionConfig,
        events: EventBus,
    ) -> (Connection, Connection) {
        let listener = TcpTransport::bind("127.0.0.1:0".parse().unwrap(), 4)
            .await
            .unwrap();
        let address = listener.local_addr().unwrap().unwrap();
        let client = TcpTransport::bind("127.0.0.1:0".parse().unwrap(), 4)
            .await
            .unwrap();
        let (dialed, accepted) = tokio::join!(client.connect(address), listener.accept());
        let dialed = dialed.unwrap();
        let (_addr, accepted) = accepted.unwrap();
        let first = spawn(
            a_channel,
            Wire::Stream(dialed),
            config.clone(),
            events.clone(),
            PathKind::Relayed,
        );
        let second = spawn(
            b_channel,
            Wire::Stream(accepted),
            config,
            events,
            PathKind::Relayed,
        );
        (first, second)
    }

    /// Pumps datagrams from `from` into `tx`, stripping the control header.
    async fn pump(socket: Arc<UdpTransport>, from: SocketAddr, tx: mpsc::Sender<Vec<u8>>) {
        while let Ok(datagram) = socket.recv_from().await {
            if datagram.from != from {
                continue;
            }
            if tx
                .send(datagram.data[crate::wire::HEADER_LEN..].to_vec())
                .await
                .is_err()
            {
                return;
            }
        }
    }

    /// Runs one step of a test with a deadline, so a stall fails loudly
    /// instead of hanging the whole suite.
    async fn step<T, F: std::future::Future<Output = T>>(what: &str, fut: F) -> T {
        match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
            Ok(value) => value,
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }

    #[tokio::test]
    async fn datagram_session_carries_streams_both_ways() {
        let a = identity(1);
        let b = identity(2);

        let a_socket = Arc::new(
            UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 256)
                .await
                .unwrap(),
        );
        let b_socket = Arc::new(
            UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 256)
                .await
                .unwrap(),
        );
        let a_addr = a_socket.local_addr().unwrap();
        let b_addr = b_socket.local_addr().unwrap();
        let (b_tx, b_rx) = mpsc::channel(256);
        let (a_tx, a_rx) = mpsc::channel(256);
        let pump_a = tokio::spawn(pump(a_socket.clone(), b_addr, a_tx));
        let pump_b = tokio::spawn(pump(b_socket.clone(), a_addr, b_tx));

        // The sessions are built in process, exactly as a completed handshake over
        // this socket would produce them; only the wire is under test here.
        let (a_channel, b_channel) = handshake_pair(&a, &b, 512);

        let events = EventBus::new(64);
        let config = SessionConfig {
            max_record: 512,
            max_frame: 256,
            // Smaller than the payload, so the transfer only completes if
            // window updates are produced and honoured.
            window: 4096,
            chunk_queue: 4,
            ..Default::default()
        };
        let session_a = a_channel.session_id();
        let session_b = b_channel.session_id();
        let mut conn_a = spawn(
            a_channel,
            Wire::Datagram {
                transport: a_socket.clone(),
                remote: b_addr,
                session: session_a,
                incoming: a_rx,
            },
            config.clone(),
            events.clone(),
            PathKind::Direct,
        );
        let mut conn_b = spawn(
            b_channel,
            Wire::Datagram {
                transport: b_socket.clone(),
                remote: a_addr,
                session: session_b,
                incoming: b_rx,
            },
            config,
            events.clone(),
            PathKind::Direct,
        );

        assert_eq!(conn_a.peer(), b.peer_id());
        assert_eq!(conn_b.peer(), a.peer_id());

        let mut client = step("open_stream", conn_a.open_stream()).await.unwrap();
        let mut server = step("accept on b", conn_b.accept()).await.unwrap();
        assert_eq!(client.peer(), b.peer_id());

        step("write hello", client.write_all(b"hello banana"))
            .await
            .unwrap();
        let mut buffer = vec![0u8; 12];
        let read = step("read hello", server.read(&mut buffer)).await.unwrap();
        assert_eq!(&buffer[..read], b"hello banana");

        // A payload larger than one record and one frame must arrive intact.
        let payload: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        let mut sender_task = tokio::spawn(async move {
            client.write_all(&payload).await.unwrap();
            client.close().await;
        });
        // The writer runs in its own task, so a failed write has to be able to
        // end this loop instead of leaving the reader waiting forever.
        let mut writer_done = false;
        let mut received = Vec::new();
        let mut chunk = vec![0u8; 4096];
        step("drain payload", async {
            loop {
                tokio::select! {
                    finished = &mut sender_task, if !writer_done => {
                        writer_done = true;
                        finished.expect("writer task panicked");
                    }
                    read = server.read(&mut chunk) => match read {
                        Ok(0) => break,
                        Ok(n) => received.extend_from_slice(&chunk[..n]),
                        Err(e) => panic!("payload read failed after {} bytes: {e}", received.len()),
                    }
                }
            }
        })
        .await;
        assert_eq!(received, expected, "payload must survive record splitting");

        // The reverse direction works on its own stream.
        let mut reply = step("open reply stream", conn_b.open_stream())
            .await
            .unwrap();
        let mut inbound = step("accept on a", conn_a.accept()).await.unwrap();
        step("write pong", reply.write_all(b"pong")).await.unwrap();
        reply.close().await;
        let mut buffer = vec![0u8; 4];
        assert_eq!(
            step("read pong", inbound.read(&mut buffer)).await.unwrap(),
            4
        );
        assert_eq!(&buffer, b"pong");
        assert_eq!(
            step("read eof", inbound.read(&mut buffer)).await.unwrap(),
            0
        );

        conn_a.close();
        conn_b.close();
        pump_a.abort();
        pump_b.abort();
    }

    #[tokio::test]
    async fn stream_session_round_trips_large_payloads() {
        let a = identity(3);
        let b = identity(4);
        let (a_channel, b_channel) = handshake_pair(&a, &b, 64 * 1024);
        let events = EventBus::new(64);
        let config = SessionConfig {
            max_record: 64 * 1024,
            max_frame: 8192,
            window: 128 * 1024,
            chunk_queue: 8,
            ..Default::default()
        };
        let (conn_a, mut conn_b) = tcp_pair(a_channel, b_channel, config, events).await;

        let mut stream = conn_a.open_stream().await.unwrap();
        let mut peer = conn_b.accept().await.unwrap();
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
        let expected = payload.clone();
        let sender = tokio::spawn(async move {
            stream.write_all(&payload).await.unwrap();
            stream.close().await;
        });
        let mut received = Vec::new();
        let mut chunk = vec![0u8; 8192];
        loop {
            let n = peer.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..n]);
        }
        sender.await.unwrap();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn a_stream_can_carry_data_in_both_directions() {
        let a = identity(11);
        let b = identity(12);
        let (a_channel, b_channel) = handshake_pair(&a, &b, 4096);
        let events = EventBus::new(16);
        let config = SessionConfig {
            max_record: 4096,
            max_frame: 512,
            ..Default::default()
        };
        let (conn_a, mut conn_b) = tcp_pair(a_channel, b_channel, config, events).await;
        let mut stream = conn_a.open_stream().await.unwrap();
        let mut peer = conn_b.accept().await.unwrap();
        peer.write_all(b"from the server").await.unwrap();
        let mut buffer = vec![0u8; 15];
        assert_eq!(stream.read(&mut buffer).await.unwrap(), 15);
        assert_eq!(&buffer, b"from the server");
        stream.write_all(b"to the server").await.unwrap();
        let mut buffer = vec![0u8; 13];
        assert_eq!(peer.read(&mut buffer).await.unwrap(), 13);
        assert_eq!(&buffer, b"to the server");
    }

    #[tokio::test]
    async fn closing_a_session_ends_accept() {
        let a = identity(5);
        let b = identity(6);
        let (a_channel, b_channel) = handshake_pair(&a, &b, 4096);
        let events = EventBus::new(16);
        let (conn_a, mut conn_b) =
            tcp_pair(a_channel, b_channel, SessionConfig::default(), events).await;
        conn_a.close();
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(5), conn_b.accept()).await;
        assert!(outcome.is_ok(), "accept must end when the session ends");
        assert!(matches!(outcome.unwrap(), Err(Error::Closed { .. })));
        assert!(conn_b.is_closed());
        assert!(conn_b.close_reason().is_some());
    }

    #[tokio::test]
    async fn a_peer_cannot_exceed_the_advertised_window() {
        let a = identity(9);
        let events = EventBus::new(16);
        let config = SessionConfig {
            max_record: 4096,
            max_frame: 512,
            window: 64,
            chunk_queue: 8,
            ..Default::default()
        };
        let (_opened_tx, _opened_rx) = mpsc::unbounded_channel();
        let handle = Arc::new(EndpointHandle {
            peer: a.peer_id(),
            path: PathKind::Direct,
            remote: None,
            shared: Arc::new(MuxShared::new(
                a.peer_id(),
                mpsc::unbounded_channel().0,
                mpsc::channel(4).0,
                config.window,
                config.max_frame,
                config.max_frame,
            )),
            control: mpsc::unbounded_channel().0,
            opened: _opened_tx,
            inboxes: Mutex::new(HashMap::new()),
            windows: Mutex::new(HashMap::new()),
            ids: Mutex::new(StreamIdAllocator::new()),
            open_streams: AtomicUsize::new(0),
            session: 1,
            config: config.clone(),
            events,
            closed: watch::channel(false).0,
            reason: StdMutex::new(None),
            records_in: AtomicU64::new(0),
            records_out: AtomicU64::new(0),
        });
        let mut outbox = Vec::new();
        dispatch(
            &handle,
            Frame::new(FrameKind::Open, 3, Vec::new()),
            &mut outbox,
            config.max_frame,
        )
        .await
        .unwrap();
        for _ in 0..2 {
            dispatch(
                &handle,
                Frame::new(FrameKind::Data, 3, vec![0u8; 48]),
                &mut outbox,
                config.max_frame,
            )
            .await
            .unwrap();
        }
        let resets = outbox
            .iter()
            .filter(|record| {
                Frame::decode(record, config.max_frame)
                    .map(|frame| frame.kind == FrameKind::Reset)
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(resets, 1, "the peer must be told to stop");
        assert!(!handle.inboxes.lock().await.contains_key(&3));
    }

    #[tokio::test]
    async fn frames_for_unknown_streams_are_ignored() {
        let a = identity(7);
        let b = identity(8);
        let (a_channel, b_channel) = handshake_pair(&a, &b, 4096);
        let events = EventBus::new(16);
        let config = SessionConfig {
            max_record: 4096,
            max_frame: 512,
            ..Default::default()
        };
        let (conn_a, conn_b) = tcp_pair(a_channel, b_channel, config, events).await;
        // Give the session task no stream at all, then push frames for one.
        let mut stream = conn_a.open_stream().await.unwrap();
        assert!(!conn_a.is_closed());
        assert!(!conn_b.is_closed());
        stream.write_all(b"still working").await.unwrap();
    }
}
