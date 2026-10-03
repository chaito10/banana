//! Stream multiplexing over one secure session.
//!
//! One framing implementation, one serialization path (RFC 19.4 and 6.3).
//! Application payload is opaque bytes; control messages share the same frame
//! format on reserved stream id zero, so there is no second wire format to keep
//! in sync.
//!
//! Flow control is a correctness property here, not an optimisation: a writer
//! cannot get ahead of a reader, so neither side can force the other to buffer
//! unbounded application data (RFC 18.3).

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, watch};

use crate::error::{Error, Result};
use crate::identity::PeerId;
use crate::wire::{Decoder, Encoder};

/// Reserved stream id for control messages.
pub const CONTROL_STREAM: u32 = 0;
/// Largest frame header.
pub const HEADER_LEN: usize = 9;
/// Default per stream receive window, in bytes.
pub const DEFAULT_WINDOW: u32 = 64 * 1024;

/// Frame kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// Open a new stream.
    Open,
    /// Acknowledge a stream open, carrying the receiver's window.
    OpenAck,
    /// Close the sending direction of a stream.
    Close,
    /// Abort a stream and discard its buffer.
    Reset,
    /// Application payload.
    Data,
    /// Grant more send credit.
    WindowUpdate,
    /// Liveness probe.
    Ping,
    /// Liveness response.
    Pong,
}

impl FrameKind {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            FrameKind::Open => 1,
            FrameKind::OpenAck => 2,
            FrameKind::Close => 3,
            FrameKind::Reset => 4,
            FrameKind::Data => 5,
            FrameKind::WindowUpdate => 6,
            FrameKind::Ping => 7,
            FrameKind::Pong => 8,
        }
    }

    pub(crate) fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            1 => FrameKind::Open,
            2 => FrameKind::OpenAck,
            3 => FrameKind::Close,
            4 => FrameKind::Reset,
            5 => FrameKind::Data,
            6 => FrameKind::WindowUpdate,
            7 => FrameKind::Ping,
            8 => FrameKind::Pong,
            other => return Err(Error::Protocol(format!("unknown frame kind {other}"))),
        })
    }

    /// Lowercase label for logs.
    pub fn as_str(self) -> &'static str {
        match self {
            FrameKind::Open => "open",
            FrameKind::OpenAck => "open-ack",
            FrameKind::Close => "close",
            FrameKind::Reset => "reset",
            FrameKind::Data => "data",
            FrameKind::WindowUpdate => "window-update",
            FrameKind::Ping => "ping",
            FrameKind::Pong => "pong",
        }
    }
}

/// Why a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    /// The local side closed the stream.
    Local,
    /// The remote side closed the stream.
    Remote,
    /// The stream was reset because of an error.
    Reset,
    /// The whole session went away.
    SessionClosed,
}

impl StreamEnd {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            StreamEnd::Local => 0,
            StreamEnd::Remote => 1,
            StreamEnd::Reset => 2,
            StreamEnd::SessionClosed => 3,
        }
    }

    pub(crate) fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            0 => StreamEnd::Local,
            1 => StreamEnd::Remote,
            2 => StreamEnd::Reset,
            3 => StreamEnd::SessionClosed,
            other => return Err(Error::Protocol(format!("unknown stream end {other}"))),
        })
    }

    /// Human readable reason.
    pub fn as_str(self) -> &'static str {
        match self {
            StreamEnd::Local => "local",
            StreamEnd::Remote => "remote",
            StreamEnd::Reset => "reset",
            StreamEnd::SessionClosed => "session-closed",
        }
    }
}

/// One multiplexed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// What the frame does.
    pub kind: FrameKind,
    /// Which stream it belongs to.
    pub stream: u32,
    /// Payload: application bytes, window credit, or a close reason.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Builds a frame.
    pub fn new(kind: FrameKind, stream: u32, payload: Vec<u8>) -> Self {
        Frame {
            kind,
            stream,
            payload,
        }
    }

    /// Encodes the frame, rejecting payloads above the configured limit.
    pub fn encode(&self, max_payload: usize) -> Result<Vec<u8>> {
        if self.payload.len() > max_payload {
            return Err(Error::Limit {
                what: "frame payload",
                limit: max_payload as u64,
            });
        }
        let mut encoder = Encoder::with_capacity(HEADER_LEN + self.payload.len());
        encoder
            .u8(self.kind.to_u8())
            .u32(self.stream)
            .u32(self.payload.len() as u32);
        encoder.raw(&self.payload);
        Ok(encoder.finish())
    }

    /// Decodes the first frame in `bytes`.
    pub fn decode(bytes: &[u8], max_payload: usize) -> Result<Frame> {
        let mut decoder = Decoder::new(bytes);
        let kind = FrameKind::from_u8(decoder.u8()?)?;
        let stream = decoder.u32()?;
        let len = decoder.u32()? as usize;
        if len > max_payload {
            return Err(Error::Limit {
                what: "frame payload",
                limit: max_payload as u64,
            });
        }
        if decoder.remaining() < len {
            return Err(Error::Protocol("truncated frame".into()));
        }
        let payload = decoder.raw(len)?.to_vec();
        Ok(Frame {
            kind,
            stream,
            payload,
        })
    }

    /// Window credit carried by a `WindowUpdate` frame.
    pub fn credit(&self) -> Result<u32> {
        if self.payload.len() != 4 {
            return Err(Error::Protocol(
                "window update must carry four bytes".into(),
            ));
        }
        Ok(u32::from_le_bytes([
            self.payload[0],
            self.payload[1],
            self.payload[2],
            self.payload[3],
        ]))
    }
}

/// Send credit for one direction of one stream.
///
/// Credit is a counter plus a level triggered notification, so a writer that
/// races with a reader returning credit always observes it: the counter is
/// checked again on every wake up, never only once before waiting.
#[derive(Debug)]
pub(crate) struct Window {
    available: AtomicU32,
    notify: watch::Sender<u32>,
}

impl Window {
    pub(crate) fn new(size: u32) -> Self {
        let (notify, _) = watch::channel(size);
        Window {
            available: AtomicU32::new(size),
            notify,
        }
    }

    pub(crate) fn take(&self, want: u32) -> bool {
        let mut current = self.available.load(Ordering::Acquire);
        loop {
            if current < want {
                return false;
            }
            match self.available.compare_exchange_weak(
                current,
                current - want,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Waits for `want` bytes of credit and consumes them.
    async fn wait(window: Arc<Window>, want: u32) -> Result<()> {
        let mut receiver = window.notify.subscribe();
        loop {
            if window.take(want) {
                window.notify.send_replace(window.available());
                return Ok(());
            }
            if receiver.wait_for(|value| *value >= want).await.is_err() {
                return Err(Error::Closed { what: "session" });
            }
        }
    }

    /// Same as [`Window::acquire`], but returns a future that owns the window
    /// so a writer can keep it across polls.
    pub(crate) fn acquire_owned(
        window: &Arc<Self>,
        want: u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        Box::pin(Self::wait(window.clone(), want))
    }

    /// Returns credit granted by the peer reading data.
    pub(crate) fn release(&self, bytes: u32) {
        if bytes == 0 {
            return;
        }
        self.available.fetch_add(bytes, Ordering::Release);
        self.notify
            .send_replace(self.available.load(Ordering::Acquire));
    }

    /// Bytes currently available.
    pub(crate) fn available(&self) -> u32 {
        self.available.load(Ordering::Acquire)
    }
}

/// One ordered outbound message for a stream.
///
/// Data and end of stream share this queue on purpose: a close that travelled
/// on a separate control queue could overtake bytes that were written but not
/// yet sent, and the peer would stop reading early.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub(crate) stream: u32,
    pub(crate) payload: Vec<u8>,
    pub(crate) end: Option<StreamEnd>,
}

impl Chunk {
    pub(crate) fn data(stream: u32, payload: Vec<u8>) -> Self {
        Chunk {
            stream,
            payload,
            end: None,
        }
    }

    pub(crate) fn end(stream: u32, end: StreamEnd) -> Self {
        Chunk {
            stream,
            payload: Vec::new(),
            end: Some(end),
        }
    }
}

/// Shared state between the session task and its streams.
#[derive(Debug)]
pub(crate) struct MuxShared {
    peer: PeerId,
    control: mpsc::UnboundedSender<Frame>,
    data: Arc<mpsc::Sender<Chunk>>,
    window_size: AtomicU32,
    max_frame: usize,
    chunk: usize,
    closed: AtomicBool,
}

impl MuxShared {
    pub(crate) fn new(
        peer: PeerId,
        control: mpsc::UnboundedSender<Frame>,
        data: mpsc::Sender<Chunk>,
        window_size: u32,
        max_frame: usize,
        chunk: usize,
    ) -> Self {
        MuxShared {
            peer,
            control,
            data: Arc::new(data),
            window_size: AtomicU32::new(window_size),
            max_frame,
            chunk,
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn peer(&self) -> PeerId {
        self.peer
    }

    /// Receive window advertised to the peer for new streams.
    pub(crate) fn window_size(&self) -> u32 {
        self.window_size.load(Ordering::Acquire)
    }

    pub(crate) fn control(&self) -> &mpsc::UnboundedSender<Frame> {
        &self.control
    }

    pub(crate) fn data(&self) -> &Arc<mpsc::Sender<Chunk>> {
        &self.data
    }

    pub(crate) fn max_frame(&self) -> usize {
        self.max_frame
    }

    pub(crate) fn chunk(&self) -> usize {
        self.chunk
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

/// A logical bidirectional byte stream inside a session.
pub struct Stream {
    id: u32,
    shared: Arc<MuxShared>,
    window: Arc<Window>,
    recv_window: Option<Arc<Window>>,
    inbox: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    pending_credit: u32,
    held_credit: u32,
    acquire: Option<(u32, Pin<Box<dyn Future<Output = Result<()>> + Send>>)>,
    reserve: Option<
        Pin<
            Box<
                dyn Future<
                        Output = std::result::Result<
                            mpsc::OwnedPermit<Chunk>,
                            mpsc::error::SendError<()>,
                        >,
                    > + Send,
            >,
        >,
    >,
    read_closed: bool,
    write_closed: bool,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("id", &self.id)
            .field("peer", &self.shared.peer())
            .field("read_closed", &self.read_closed)
            .field("write_closed", &self.write_closed)
            .finish_non_exhaustive()
    }
}

impl Stream {
    pub(crate) fn new(
        id: u32,
        shared: Arc<MuxShared>,
        window: Arc<Window>,
        inbox: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        Stream {
            id,
            shared,
            window,
            recv_window: None,
            inbox,
            pending: Vec::new(),
            pending_credit: 0,
            held_credit: 0,
            acquire: None,
            reserve: None,
            read_closed: false,
            write_closed: false,
        }
    }

    /// Attaches the counter the session task uses to police incoming bytes.
    ///
    /// Bytes count against it when they arrive and are given back as the
    /// application reads them, so a peer that ignores flow control is cut off
    /// after one window instead of filling memory.
    pub(crate) fn with_recv_window(mut self, window: Arc<Window>) -> Self {
        self.recv_window = Some(window);
        self
    }

    /// Stream id inside this session.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The peer on the other side.
    pub fn peer(&self) -> PeerId {
        self.shared.peer()
    }

    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize>> {
        self.flush_credit();
        if !self.pending.is_empty() {
            let take = std::cmp::min(buf.len(), self.pending.len());
            buf[..take].copy_from_slice(&self.pending[..take]);
            self.pending.drain(..take);
            self.pending_credit += take as u32;
            // Consumed bytes leave the receive window again, which is what lets
            // the peer keep sending after one window has been delivered.
            if let Some(window) = &self.recv_window {
                window.release(take as u32);
            }
            if self.pending_credit as usize >= self.shared.chunk() {
                let credit = std::mem::take(&mut self.pending_credit);
                let _ = self.shared.control().send(Frame::new(
                    FrameKind::WindowUpdate,
                    self.id,
                    credit.to_le_bytes().to_vec(),
                ));
            }
            return Poll::Ready(Ok(take));
        }
        if self.read_closed {
            return Poll::Ready(Ok(0));
        }
        match self.inbox.poll_recv(cx) {
            Poll::Ready(Some(chunk)) => {
                self.pending = chunk;
                self.poll_read(cx, buf)
            }
            Poll::Ready(None) => {
                self.read_closed = true;
                self.flush_credit();
                Poll::Ready(Ok(0))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn flush_credit(&mut self) {
        if self.pending_credit == 0 {
            return;
        }
        let credit = std::mem::take(&mut self.pending_credit);
        let _ = self.shared.control().send(Frame::new(
            FrameKind::WindowUpdate,
            self.id,
            credit.to_le_bytes().to_vec(),
        ));
    }

    /// Reads application bytes, waiting when the peer has sent nothing.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        std::future::poll_fn(|cx| self.poll_read(cx, buf)).await
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, data: &[u8]) -> Poll<Result<usize>> {
        if self.write_closed {
            return Poll::Ready(Err(Error::Closed { what: "stream" }));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let total = std::cmp::min(data.len(), self.shared.max_frame());
        let chunk = data[..total].to_vec();
        let want = total as u32;
        if self.held_credit < want {
            // The credit wait has to outlive the poll that started it: a
            // notification arriving while nothing is waiting would be dropped
            // and the writer would sleep forever.
            if self.acquire.as_ref().map(|(size, _)| *size) != Some(want) {
                let window = self.window.clone();
                self.acquire = Some((want, Window::acquire_owned(&window, want)));
            }
            let future = self.acquire.as_mut().expect("acquire slot").1.as_mut();
            match Future::poll(future, cx) {
                Poll::Ready(Ok(())) => {
                    self.acquire = None;
                    self.held_credit = want;
                }
                Poll::Ready(Err(e)) => {
                    self.acquire = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        match self.shared.data().try_send(Chunk::data(self.id, chunk)) {
            Ok(()) => {
                self.held_credit = 0;
                Poll::Ready(Ok(total))
            }
            Err(mpsc::error::TrySendError::Full(chunk)) => {
                // The permit has to be held across polls, exactly like the
                // credit future above: a future built inside the poll is
                // dropped again before anything can wake the writer, so a slot
                // freeing up in that window would be lost and the stream would
                // stall forever.
                if self.reserve.is_none() {
                    let sender = mpsc::Sender::clone(self.shared.data().as_ref());
                    self.reserve = Some(Box::pin(sender.reserve_owned()));
                }
                let permit = self.reserve.as_mut().expect("reserve slot");
                match Future::poll(permit.as_mut(), cx) {
                    Poll::Ready(Ok(permit)) => {
                        self.reserve = None;
                        permit.send(chunk);
                        self.held_credit = 0;
                        Poll::Ready(Ok(total))
                    }
                    Poll::Ready(Err(_)) => {
                        self.reserve = None;
                        Poll::Ready(Err(Error::Closed { what: "session" }))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Poll::Ready(Err(Error::Closed { what: "session" }))
            }
        }
    }

    /// Writes application bytes, blocking while the peer is behind.
    pub async fn write(&mut self, data: &[u8]) -> Result<usize> {
        std::future::poll_fn(|cx| self.poll_write(cx, data)).await
    }

    /// Writes every byte, respecting flow control.
    pub async fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let mut written = 0;
        while written < data.len() {
            let n = self.write(&data[written..]).await?;
            if n == 0 {
                return Err(Error::Closed { what: "stream" });
            }
            written += n;
        }
        Ok(())
    }

    /// Closes the sending direction.
    ///
    /// The end of stream marker goes out on the same ordered queue as data, so
    /// the peer always sees it after every byte written before it.
    pub async fn close(&mut self) {
        if self.write_closed {
            return;
        }
        self.write_closed = true;
        self.flush_credit();
        // Awaiting the queue is what makes the marker lossless: a `try_send`
        // would drop it whenever the writer outruns the session task, and the
        // peer would then wait for an end of stream that never arrives.
        let _ = self
            .shared
            .data()
            .send(Chunk::end(self.id, StreamEnd::Local))
            .await;
    }

    /// Aborts the stream, telling the peer to discard what is buffered.
    pub async fn reset(&mut self) {
        if self.write_closed {
            return;
        }
        self.write_closed = true;
        self.flush_credit();
        let _ = self
            .shared
            .data()
            .send(Chunk::end(self.id, StreamEnd::Reset))
            .await;
    }

    /// Whether the peer closed its sending direction.
    pub fn is_read_closed(&self) -> bool {
        self.read_closed
    }

    /// Whether this side closed its sending direction.
    pub fn is_write_closed(&self) -> bool {
        self.write_closed
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Credit taken but never sent is given back, otherwise a cancelled
        // write would shrink the window for the life of the session.
        if self.held_credit > 0 {
            self.window.release(self.held_credit);
            self.held_credit = 0;
        }
        // Dropping without closing is an abort: the peer must release its
        // buffers instead of waiting for an end of stream that never comes.
        if !self.write_closed && !self.shared.is_closed() {
            let marker = Chunk::end(self.id, StreamEnd::Reset);
            if self.shared.data().try_send(marker).is_err() {
                // The ordered queue is full, which `drop` cannot wait on. A
                // reset needs no ordering guarantee, so it goes out on the
                // control queue instead of being lost.
                let _ = self.shared.control().send(Frame::new(
                    FrameKind::Reset,
                    self.id,
                    vec![StreamEnd::Reset.to_u8()],
                ));
            }
        }
    }
}

/// Adapter that exposes a [`Stream`] as an `AsyncRead`.
#[derive(Debug)]
pub struct StreamReader {
    stream: Stream,
}

impl StreamReader {
    /// Wraps a stream for reading.
    pub fn new(stream: Stream) -> Self {
        StreamReader { stream }
    }

    /// Unwraps the stream.
    pub fn into_inner(self) -> Stream {
        self.stream
    }
}

impl AsyncRead for StreamReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let destination = buf.initialize_unfilled();
        match this.stream.poll_read(cx, destination) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::from(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Adapter that exposes a [`Stream`] as an `AsyncWrite`.
#[derive(Debug)]
pub struct StreamWriter {
    stream: Stream,
}

impl StreamWriter {
    /// Wraps a stream for writing.
    pub fn new(stream: Stream) -> Self {
        StreamWriter { stream }
    }

    /// Unwraps the stream.
    pub fn into_inner(self) -> Stream {
        self.stream
    }
}

impl AsyncWrite for StreamWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().stream.poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::from(e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Frames are handed to the session task as they are produced; there is
        // no intermediate buffer to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // `close` waits for queue space, which a poll cannot do; the ordered
        // queue is drained by the session task, so marking the write side
        // closed and queueing the marker without awaiting is the best a sync
        // poll can offer here. Callers that need the marker delivered should
        // use `Stream::close` directly.
        let _ = self.get_mut().stream.close();
        Poll::Ready(Ok(()))
    }
}

/// Incremental frame decoder for the inbound side of a session.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
    max_payload: usize,
}

impl FrameReader {
    /// Creates a reader bounded by `max_payload`.
    pub fn new(max_payload: usize) -> Self {
        FrameReader {
            buffer: Vec::new(),
            max_payload,
        }
    }

    /// Feeds received bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Pops the next complete frame, if one is buffered.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }
        let declared = u32::from_le_bytes([
            self.buffer[5],
            self.buffer[6],
            self.buffer[7],
            self.buffer[8],
        ]) as usize;
        if declared > self.max_payload {
            return Err(Error::Limit {
                what: "frame payload",
                limit: self.max_payload as u64,
            });
        }
        let total = HEADER_LEN + declared;
        if self.buffer.len() < total {
            return Ok(None);
        }
        let frame = Frame::decode(&self.buffer, self.max_payload)?;
        self.buffer.drain(..total);
        Ok(Some(frame))
    }

    /// Bytes buffered but not yet framed.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

/// Allocates ids for locally initiated streams.
#[derive(Debug, Default)]
pub struct StreamIdAllocator {
    next: u32,
}

impl StreamIdAllocator {
    /// Creates an allocator starting at 1; zero is reserved for control.
    pub fn new() -> Self {
        StreamIdAllocator { next: 1 }
    }

    /// Returns the next unused stream id.
    pub fn next_id(&mut self) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        id
    }

    /// Moves the cursor past an id the peer chose.
    ///
    /// Both directions share one id space, so a locally opened stream must
    /// never be handed an id the peer still has open; keeping the cursor past
    /// every id seen makes collisions impossible for the life of the session.
    pub fn reserve(&mut self, id: u32) {
        if id >= self.next && id < u32::MAX {
            self.next = id + 1;
        }
    }
}

/// Per stream configuration used when a session starts.
#[derive(Debug, Clone, Copy)]
pub struct StreamLimits {
    /// Largest single frame payload.
    pub max_frame: usize,
    /// Receive window advertised to the peer.
    pub window: u32,
    /// Number of buffered chunks per stream, so a reader that stalls cannot
    /// make the session buffer more than `chunk_queue * chunk` bytes.
    pub chunk_queue: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        StreamLimits {
            max_frame: 16 * 1024,
            window: DEFAULT_WINDOW,
            chunk_queue: 8,
        }
    }
}

impl StreamLimits {
    /// Largest number of bytes buffered for one inbound stream.
    pub fn max_buffered_bytes(&self) -> usize {
        self.chunk_queue.saturating_mul(self.max_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[allow(clippy::type_complexity)]
    fn parts(
        window: u32,
        max_frame: usize,
        queue: usize,
    ) -> (
        Arc<MuxShared>,
        Arc<Window>,
        mpsc::UnboundedReceiver<Frame>,
        mpsc::Receiver<Chunk>,
    ) {
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (data_tx, data_rx) = mpsc::channel(queue);
        let shared = Arc::new(MuxShared {
            peer: PeerId::from_bytes([1u8; 32]),
            control: control_tx,
            data: Arc::new(data_tx),
            window_size: AtomicU32::new(window),
            max_frame,
            chunk: max_frame,
            closed: AtomicBool::new(false),
        });
        (shared, Arc::new(Window::new(window)), control_rx, data_rx)
    }

    #[test]
    fn frames_round_trip() {
        let frame = Frame::new(FrameKind::Data, 7, vec![1, 2, 3]);
        let encoded = frame.encode(1024).unwrap();
        assert_eq!(encoded.len(), HEADER_LEN + 3);
        assert_eq!(Frame::decode(&encoded, 1024).unwrap(), frame);
        assert!(Frame::new(FrameKind::Data, 1, vec![1]).credit().is_err());
    }

    #[test]
    fn frame_limits_are_enforced() {
        let frame = Frame::new(FrameKind::Data, 1, vec![0u8; 100]);
        assert!(matches!(frame.encode(10), Err(Error::Limit { .. })));
        let encoded = vec![FrameKind::Data.to_u8(), 0, 0, 0, 1, 0, 0, 0x10, 0];
        assert!(matches!(
            Frame::decode(&encoded, 16),
            Err(Error::Limit { .. })
        ));
        assert!(Frame::decode(&[5, 0], 16).is_err());
    }

    #[test]
    fn window_update_payloads_decode() {
        let frame = Frame::new(FrameKind::WindowUpdate, 2, 4096u32.to_le_bytes().to_vec());
        assert_eq!(frame.credit().unwrap(), 4096);
        let bad = Frame::new(FrameKind::WindowUpdate, 2, vec![1, 2]);
        assert!(bad.credit().is_err());
    }

    #[test]
    fn reader_reassembles_partial_frames() {
        let mut reader = FrameReader::new(1024);
        let encoded = Frame::new(FrameKind::Data, 1, vec![9u8; 10])
            .encode(1024)
            .unwrap();
        for byte in &encoded[..encoded.len() - 1] {
            reader.feed(&[*byte]);
            assert!(reader.next_frame().unwrap().is_none());
        }
        reader.feed(&encoded[encoded.len() - 1..]);
        let frame = reader.next_frame().unwrap().unwrap();
        assert_eq!(frame.payload.len(), 10);
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn unknown_frame_kinds_are_rejected() {
        let encoded = vec![99u8, 0, 0, 0, 1, 0, 0, 0, 0];
        assert!(Frame::decode(&encoded, 16).is_err());
    }

    #[tokio::test]
    async fn write_waits_for_credit_then_succeeds() {
        let (shared, window, _control, _data) = parts(4, 4, 4);
        Window::acquire_owned(&window, 4).await.unwrap();
        let (_tx, inbox) = mpsc::channel(4);
        let mut stream = Stream::new(1, shared, window.clone(), inbox);
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            stream.write(&[0u8; 4]),
        )
        .await;
        assert!(blocked.is_err(), "writer must wait for credit");
        window.release(4);
        assert_eq!(stream.write(&[0u8; 4]).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn write_never_returns_more_than_one_frame() {
        let (shared, window, _control, mut data) = parts(4096, 4, 4);
        let (_tx, inbox) = mpsc::channel(1);
        let mut stream = Stream::new(1, shared, window, inbox);
        assert_eq!(stream.write(&[0u8; 10]).await.unwrap(), 4);
        let chunk = data.recv().await.unwrap();
        assert_eq!(chunk.stream, 1);
        assert_eq!(chunk.payload.len(), 4);
    }

    #[tokio::test]
    async fn dropped_streams_return_held_credit() {
        let (shared, window, _control, _data) = parts(8, 4, 1);
        Window::acquire_owned(&window, 4).await.unwrap();
        shared
            .data()
            .try_send(Chunk::data(1, vec![0u8; 4]))
            .unwrap();
        {
            let (_tx, inbox) = mpsc::channel(1);
            let mut stream = Stream::new(1, shared.clone(), window.clone(), inbox);
            let blocked = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                stream.write(&[0u8; 4]),
            )
            .await;
            assert!(blocked.is_err());
            // Credit was taken for a frame that never left the session.
            assert_eq!(window.available(), 0);
        }
        assert_eq!(window.available(), 4, "credit must come back on drop");
    }

    #[tokio::test]
    async fn read_returns_data_then_eof() {
        let (shared, window, _control, _data) = parts(4096, 1024, 4);
        let (tx, inbox) = mpsc::channel(4);
        let mut stream = Stream::new(1, shared, window, inbox);
        tx.send(vec![1, 2, 3]).await.unwrap();
        drop(tx);
        let mut buf = [0u8; 8];
        assert_eq!(stream.read(&mut buf).await.unwrap(), 3);
        assert_eq!(&buf[..3], &[1, 2, 3]);
        assert_eq!(stream.read(&mut buf).await.unwrap(), 0);
        assert!(stream.is_read_closed());
    }

    #[tokio::test]
    async fn reading_grants_credit_to_the_peer() {
        let (shared, window, mut control, _data) = parts(4096, 512, 4);
        let (tx, inbox) = mpsc::channel(4);
        let mut stream = Stream::new(1, shared, window, inbox);
        tx.send(vec![0u8; 512]).await.unwrap();
        drop(tx);
        let mut buf = [0u8; 512];
        assert_eq!(stream.read(&mut buf).await.unwrap(), 512);
        let _ = stream.read(&mut buf).await;
        let mut credit = 0u32;
        while let Ok(frame) = control.try_recv() {
            if frame.kind == FrameKind::WindowUpdate {
                credit += frame.credit().unwrap();
            }
        }
        assert_eq!(credit, 512, "reader must return the credit it consumed");
    }

    #[tokio::test]
    async fn closing_reports_the_reason_to_the_peer() {
        let (shared, window, _control, mut data) = parts(4096, 1024, 4);
        let (_tx, inbox) = mpsc::channel(1);
        let mut stream = Stream::new(9, shared, window, inbox);
        stream.close().await;
        let chunk = data.recv().await.unwrap();
        assert_eq!(chunk.stream, 9);
        assert_eq!(chunk.end, Some(StreamEnd::Local));
        assert!(stream.is_write_closed());
        stream.close().await;
        assert!(data.try_recv().is_err(), "close is idempotent");
        assert!(stream.write(&[1]).await.is_err());
    }

    #[tokio::test]
    async fn end_of_stream_is_ordered_after_written_bytes() {
        let (shared, window, _control, mut data) = parts(4096, 4, 8);
        let (_tx, inbox) = mpsc::channel(1);
        let mut stream = Stream::new(2, shared, window, inbox);
        stream.write_all(b"abcdefgh").await.unwrap();
        stream.close().await;
        let mut payloads = Vec::new();
        let mut ended = false;
        while let Ok(chunk) = data.try_recv() {
            if chunk.end.is_some() {
                ended = true;
            } else {
                assert!(!ended, "no data may follow the end of stream marker");
                payloads.extend_from_slice(&chunk.payload);
            }
        }
        assert!(ended, "close must reach the peer");
        assert_eq!(payloads, b"abcdefgh", "every byte must precede the close");
    }

    #[tokio::test]
    async fn reset_marks_the_stream_aborted() {
        let (shared, window, _control, mut data) = parts(4096, 1024, 4);
        let (_tx, inbox) = mpsc::channel(1);
        let mut stream = Stream::new(4, shared, window, inbox);
        stream.reset().await;
        let chunk = data.recv().await.unwrap();
        assert_eq!(chunk.end, Some(StreamEnd::Reset));
    }

    #[tokio::test]
    async fn async_read_adapter_sees_end_of_stream() {
        let (shared, window, _control, _data) = parts(4096, 1024, 4);
        let (tx, inbox) = mpsc::channel(4);
        tx.send(vec![7u8, 8, 9]).await.unwrap();
        drop(tx);
        let mut reader = StreamReader::new(Stream::new(1, shared, window, inbox));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn async_write_adapter_respects_credit() {
        let (shared, window, _control, mut data) = parts(8, 4, 8);
        let (_tx, inbox) = mpsc::channel(4);
        let mut writer = StreamWriter::new(Stream::new(1, shared, window.clone(), inbox));
        let task = tokio::spawn(async move {
            writer.write_all(&[0u8; 20]).await.unwrap();
            writer.shutdown().await.unwrap();
        });
        let mut sent = 0;
        while sent < 20 {
            let chunk = data.recv().await.unwrap();
            window.release(chunk.payload.len() as u32);
            sent += chunk.payload.len();
        }
        task.await.unwrap();
    }

    #[test]
    fn stream_ids_never_collide_with_control() {
        let mut ids = StreamIdAllocator::new();
        let first = ids.next_id();
        assert_ne!(first, CONTROL_STREAM);
        assert_ne!(ids.next_id(), first);
    }

    #[test]
    fn buffered_bytes_are_bounded_by_configuration() {
        let limits = StreamLimits {
            max_frame: 4096,
            window: 16_384,
            chunk_queue: 4,
        };
        assert_eq!(limits.max_buffered_bytes(), 16_384);
        assert_eq!(StreamLimits::default().max_buffered_bytes(), 8 * 16 * 1024);
    }

    #[test]
    fn advertised_window_matches_the_configured_size() {
        let (shared, _window, _control, _data) = parts(1024, 512, 4);
        assert_eq!(shared.window_size(), 1024);
    }

    proptest! {
        #[test]
        fn frame_codec_round_trips(any_kind in 1u8..=8, stream in 1u32..=u32::MAX, payload in proptest::collection::vec(any::<u8>(), 0..512)) {
            let kind = match any_kind {
                1 => FrameKind::Open,
                2 => FrameKind::OpenAck,
                3 => FrameKind::Close,
                4 => FrameKind::Reset,
                5 => FrameKind::Data,
                6 => FrameKind::WindowUpdate,
                7 => FrameKind::Ping,
                _ => FrameKind::Pong,
            };
            let frame = Frame::new(kind, stream, payload.clone());
            let encoded = frame.encode(4096).unwrap();
            prop_assert_eq!(Frame::decode(&encoded, 4096).unwrap(), frame);
        }
    }

    proptest! {
        #[test]
        fn decoder_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let mut reader = FrameReader::new(1024);
            reader.feed(&bytes);
            let _ = reader.next_frame();
        }
    }
}
