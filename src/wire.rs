//! Canonical wire codec.
//!
//! Every byte that crosses a socket boundary is produced and consumed here.
//! The RFC requires exactly one framing implementation and one serialization
//! path per protocol, so no other module is allowed to hand-roll encoding.
//! The decoder is bounds checked and never panics, which makes it a safe
//! fuzz target.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::error::{Error, Result};
use crate::identity::PeerId;

/// Wire protocol magic: `BN`.
pub const MAGIC: u16 = 0x424E;
/// Wire protocol version.
pub const VERSION: u8 = 1;

/// Length of the datagram control header: magic, version, kind, session id.
pub const HEADER_LEN: usize = 8;

/// Largest datagram the runtime sends or accepts. Keeps traffic MTU safe.
pub const MAX_DATAGRAM: usize = 1200;
/// Largest number of addresses a single record may carry.
pub const MAX_ADDRESSES: usize = 8;
/// Number of peers returned by a single routing query.
pub const MAX_NODES: usize = 20;

/// Datagram kinds.
pub mod kind {
    /// Distributed hash table messages.
    pub const DHT: u8 = 1;
    /// Reachability probing and hole punch coordination.
    pub const NAT: u8 = 2;
    /// Noise handshake messages and encrypted records.
    pub const NOISE: u8 = 3;
    /// Resynchronisation request for an unknown Noise session id.
    pub const RESYNC: u8 = 4;
}

/// The datagram control header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Message kind, see [`kind`].
    pub kind: u8,
    /// Noise session id, or zero for connectionless kinds.
    pub session: u32,
}

impl Header {
    /// Builds a header.
    pub const fn new(kind: u8, session: u32) -> Self {
        Header { kind, session }
    }

    /// Encodes the header.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        out[2] = VERSION;
        out[3] = self.kind;
        out[4..8].copy_from_slice(&self.session.to_le_bytes());
        out
    }

    /// Decodes a header, rejecting foreign or oversized datagrams.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() > MAX_DATAGRAM {
            return Err(Error::Limit {
                what: "datagram",
                limit: MAX_DATAGRAM as u64,
            });
        }
        if buf.len() < HEADER_LEN {
            return Err(Error::Protocol("datagram shorter than header".into()));
        }
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != MAGIC {
            return Err(Error::Protocol("bad wire magic".into()));
        }
        if buf[2] != VERSION {
            return Err(Error::Unsupported(format!("wire version {}", buf[2])));
        }
        let session = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        Ok(Header {
            kind: buf[3],
            session,
        })
    }
}

/// Append-only encoder. Every method returns `&mut Self` so calls chain.
#[derive(Debug, Default)]
pub struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    /// Starts an encoder with reserved capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Encoder {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Number of bytes written so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Appends one byte.
    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }

    /// Appends a little endian `u16`.
    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends a little endian `u32`.
    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends a little endian `u64`.
    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends raw bytes without a length prefix.
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    /// Appends a length prefixed byte string.
    ///
    /// Fails loudly when the string exceeds `max`, so a bug in the caller
    /// cannot become an oversized datagram.
    pub fn prefixed(&mut self, bytes: &[u8], max: usize) -> Result<&mut Self> {
        if bytes.len() > max {
            return Err(Error::Limit {
                what: "prefixed field",
                limit: max as u64,
            });
        }
        self.u16(bytes.len() as u16);
        self.buf.extend_from_slice(bytes);
        Ok(self)
    }

    /// Appends a count prefixed byte string.
    pub fn counted(&mut self, bytes: &[u8], max: usize) -> Result<&mut Self> {
        if bytes.len() > max {
            return Err(Error::Limit {
                what: "counted field",
                limit: max as u64,
            });
        }
        self.u8(bytes.len() as u8);
        self.buf.extend_from_slice(bytes);
        Ok(self)
    }

    /// Appends a peer id.
    pub fn peer_id(&mut self, id: PeerId) -> &mut Self {
        self.buf.extend_from_slice(id.as_bytes());
        self
    }

    /// Appends a socket address in the canonical compact form.
    pub fn addr(&mut self, addr: SocketAddr) -> &mut Self {
        match addr.ip() {
            IpAddr::V4(v4) => {
                self.u8(4);
                self.u16(addr.port());
                self.raw(&v4.octets());
            }
            IpAddr::V6(v6) => {
                self.u8(6);
                self.u16(addr.port());
                self.raw(&v6.octets());
            }
        }
        self
    }

    /// Appends a list of addresses, capped at [`MAX_ADDRESSES`].
    pub fn addrs(&mut self, addrs: &[SocketAddr]) -> Result<&mut Self> {
        if addrs.len() > MAX_ADDRESSES {
            return Err(Error::Limit {
                what: "addresses",
                limit: MAX_ADDRESSES as u64,
            });
        }
        self.u8(addrs.len() as u8);
        for addr in addrs {
            self.addr(*addr);
        }
        Ok(self)
    }

    /// Finishes encoding.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Finishes encoding with the datagram header prepended.
    pub fn packet(self, kind: u8, session: u32) -> Vec<u8> {
        let mut out = Header::new(kind, session).encode().to_vec();
        out.extend_from_slice(&self.buf);
        out
    }
}

/// Bounds checked reader. Every read validates remaining length first.
#[derive(Debug)]
pub struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Wraps a byte slice.
    pub fn new(buf: &'a [u8]) -> Self {
        Decoder { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Whether the whole input has been consumed.
    pub fn is_done(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Protocol("truncated message".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// Reads one byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Reads a little endian `u32`.
    pub fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a little endian `u64`.
    pub fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(arr))
    }

    /// Reads `n` raw bytes.
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }

    /// Reads a length prefixed byte string, rejecting anything above `max`.
    pub fn prefixed(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(Error::Limit {
                what: "prefixed field",
                limit: max as u64,
            });
        }
        self.take(len)
    }

    /// Reads a count prefixed byte string, rejecting anything above `max`.
    pub fn counted(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.u8()? as usize;
        if len > max {
            return Err(Error::Limit {
                what: "counted field",
                limit: max as u64,
            });
        }
        self.take(len)
    }

    /// Reads a peer id.
    pub fn peer_id(&mut self) -> Result<PeerId> {
        let bytes = self.take(32)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(PeerId::from_bytes(arr))
    }

    /// Reads a socket address.
    pub fn addr(&mut self) -> Result<SocketAddr> {
        let family = self.u8()?;
        let port = self.u16()?;
        let ip = match family {
            4 => {
                let bytes = self.take(4)?;
                let mut octets = [0u8; 4];
                octets.copy_from_slice(bytes);
                IpAddr::V4(Ipv4Addr::from(octets))
            }
            6 => {
                let bytes = self.take(16)?;
                let mut octets = [0u8; 16];
                octets.copy_from_slice(bytes);
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            other => {
                return Err(Error::Protocol(format!("unknown address family {other}")));
            }
        };
        Ok(SocketAddr::new(ip, port))
    }

    /// Reads a list of addresses, capped at [`MAX_ADDRESSES`].
    pub fn addrs(&mut self) -> Result<Vec<SocketAddr>> {
        let count = self.u8()? as usize;
        if count > MAX_ADDRESSES {
            return Err(Error::Limit {
                what: "addresses",
                limit: MAX_ADDRESSES as u64,
            });
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.addr()?);
        }
        Ok(out)
    }

    /// Rejects trailing bytes, so a decoder never silently ignores data.
    pub fn finish(&self) -> Result<()> {
        if self.is_done() {
            Ok(())
        } else {
            Err(Error::Protocol(format!(
                "{} trailing bytes",
                self.remaining()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_addr() -> SocketAddr {
        "203.0.113.7:49737".parse().unwrap()
    }

    #[test]
    fn header_round_trips() {
        let header = Header::new(kind::DHT, 0xdead_beef);
        let encoded = header.encode();
        assert_eq!(encoded.len(), HEADER_LEN);
        assert_eq!(Header::decode(&encoded).unwrap(), header);
    }

    #[test]
    fn header_rejects_junk() {
        assert!(Header::decode(&[]).is_err());
        assert!(Header::decode(&[0; 4]).is_err());
        let mut bad = Header::new(kind::DHT, 0).encode().to_vec();
        bad[0] = 0;
        assert!(Header::decode(&bad).is_err());
        let mut future = Header::new(kind::DHT, 0).encode().to_vec();
        future[2] = VERSION + 1;
        assert!(matches!(
            Header::decode(&future),
            Err(Error::Unsupported(_))
        ));
        let huge = vec![0u8; MAX_DATAGRAM + 1];
        assert!(matches!(Header::decode(&huge), Err(Error::Limit { .. })));
    }

    #[test]
    fn scalars_round_trip() {
        let mut enc = Encoder::with_capacity(32);
        enc.u8(1)
            .u16(2)
            .u32(3)
            .u64(4)
            .raw(&[5, 6])
            .peer_id(PeerId::from_bytes([9u8; 32]));
        let bytes = enc.finish();
        let mut dec = Decoder::new(&bytes);
        assert_eq!(dec.u8().unwrap(), 1);
        assert_eq!(dec.u16().unwrap(), 2);
        assert_eq!(dec.u32().unwrap(), 3);
        assert_eq!(dec.u64().unwrap(), 4);
        assert_eq!(dec.raw(2).unwrap(), &[5, 6]);
        assert_eq!(dec.peer_id().unwrap(), PeerId::from_bytes([9u8; 32]));
        assert!(dec.finish().is_ok());
        assert_eq!(bytes.len(), 1 + 2 + 4 + 8 + 2 + 32);
    }

    #[test]
    fn addresses_round_trip() {
        let v4 = sample_addr();
        let v6: SocketAddr = "[2001:db8::1]:49737".parse().unwrap();
        let mut enc = Encoder::with_capacity(64);
        enc.addrs(&[v4, v6]).unwrap();
        let bytes = enc.finish();
        let mut dec = Decoder::new(&bytes);
        assert_eq!(dec.addrs().unwrap(), vec![v4, v6]);
        assert!(dec.finish().is_ok());
        assert_eq!(bytes.len(), 1 + 7 + 19);
    }

    #[test]
    fn decoder_never_panics_on_garbage() {
        for len in 0..40usize {
            for fill in [0u8, 1, 255] {
                let buf = vec![fill; len];
                let mut dec = Decoder::new(&buf);
                let _ = dec.u8();
                let _ = dec.u16();
                let _ = dec.u32();
                let _ = dec.u64();
                let _ = dec.addrs();
                let _ = dec.prefixed(64);
                let _ = dec.peer_id();
                let _ = dec.finish();
            }
        }
    }

    #[test]
    fn limits_are_enforced() {
        let mut enc = Encoder::with_capacity(8);
        assert!(matches!(
            enc.addrs(&[sample_addr(); 9]),
            Err(Error::Limit {
                what: "addresses",
                ..
            })
        ));
        assert!(matches!(
            enc.prefixed(&[0u8; 100], 10),
            Err(Error::Limit { .. })
        ));
        assert!(matches!(
            enc.counted(&[0u8; 100], 10),
            Err(Error::Limit { .. })
        ));
    }

    #[test]
    fn unknown_address_family_is_rejected() {
        let bytes = [9u8, 0, 0, 1, 2];
        assert!(Decoder::new(&bytes).addr().is_err());
    }
}
