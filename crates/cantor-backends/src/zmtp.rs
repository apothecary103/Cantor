//! A minimal ZeroMQ REQ client (ZMTP 3.0, NULL security) over TCP, enough
//! to talk to svs.io backends without linking libzmq.
//!
//! Wire format (rfc.zeromq.org/spec/23): a 64-byte greeting each way, a
//! READY command each way carrying `Socket-Type`, then frames: a flags byte
//! (bit 0 MORE, bit 1 LONG, bit 2 COMMAND), a 1- or 8-byte big-endian size,
//! and the body. A REQ socket prefixes every request with an empty
//! delimiter frame and expects the same envelope on replies.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const MORE: u8 = 0x01;
const LONG: u8 = 0x02;
const COMMAND: u8 = 0x04;

/// A connected REQ socket.
#[derive(Debug)]
pub(crate) struct ReqSocket {
    stream: TcpStream,
}

/// A frame as read from the wire.
struct Frame {
    flags: u8,
    body: Vec<u8>,
}

impl ReqSocket {
    /// Connects to `tcp://host:port` (or `host:port`) and completes the
    /// ZMTP handshake within `timeout`.
    pub(crate) fn connect(endpoint: &str, timeout: Duration) -> Result<Self, String> {
        let addr = endpoint.strip_prefix("tcp://").unwrap_or(endpoint);
        let addrs: Vec<_> = std::net::ToSocketAddrs::to_socket_addrs(addr)
            .map_err(|e| format!("{endpoint}: {e}"))?
            .collect();
        let first = addrs
            .first()
            .ok_or_else(|| format!("{endpoint}: no address"))?;
        let stream =
            TcpStream::connect_timeout(first, timeout).map_err(|e| format!("{endpoint}: {e}"))?;
        stream
            .set_nodelay(true)
            .map_err(|e| format!("{endpoint}: {e}"))?;
        let mut s = Self { stream };
        s.set_timeout(timeout)?;
        s.handshake().map_err(|e| format!("{endpoint}: {e}"))?;
        Ok(s)
    }

    /// Read and write timeout for later calls.
    pub(crate) fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        self.stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| self.stream.set_write_timeout(Some(timeout)))
            .map_err(|e| e.to_string())
    }

    fn handshake(&mut self) -> Result<(), String> {
        let mut greeting = [0u8; 64];
        greeting[0] = 0xFF;
        greeting[9] = 0x7F;
        greeting[10] = 3; // major
        greeting[11] = 0; // minor
        greeting[12..16].copy_from_slice(b"NULL");
        self.write_all(&greeting)?;
        let mut peer = [0u8; 64];
        self.read_exact(&mut peer)?;
        if peer[0] != 0xFF || peer[9] & 1 != 1 {
            return Err("not a ZMTP peer".into());
        }
        if peer[10] < 3 {
            return Err(format!("peer speaks ZMTP {}.x; 3.0 is required", peer[10]));
        }
        if &peer[12..16] != b"NULL" {
            return Err("peer requires a security mechanism other than NULL".into());
        }
        let mut ready = Vec::new();
        ready.push(5);
        ready.extend_from_slice(b"READY");
        property(&mut ready, "Socket-Type", b"REQ");
        self.write_frame(COMMAND, &ready)?;
        let reply = self.read_frame()?;
        if reply.flags & COMMAND == 0 || !reply.body.starts_with(b"\x05READY") {
            return Err("peer did not send READY".into());
        }
        Ok(())
    }

    /// Sends one request message and returns the reply's body.
    pub(crate) fn request(&mut self, body: &[u8]) -> Result<Vec<u8>, String> {
        self.write_frame(MORE, &[])?;
        self.write_frame(0, body)?;
        // Reply: empty delimiter, then the message (multipart parts are
        // concatenated; svs.io sends one part).
        let delim = self.read_frame()?;
        if delim.flags & MORE == 0 || !delim.body.is_empty() {
            return Err("reply has no REQ envelope".into());
        }
        let mut out = Vec::new();
        loop {
            let f = self.read_frame()?;
            if f.flags & COMMAND != 0 {
                continue;
            }
            out.extend_from_slice(&f.body);
            if f.flags & MORE == 0 {
                return Ok(out);
            }
        }
    }

    fn write_frame(&mut self, flags: u8, body: &[u8]) -> Result<(), String> {
        let mut head = Vec::with_capacity(9);
        if body.len() > 255 {
            head.push(flags | LONG);
            head.extend_from_slice(&(body.len() as u64).to_be_bytes());
        } else {
            head.push(flags);
            head.push(body.len() as u8);
        }
        self.write_all(&head)?;
        self.write_all(body)
    }

    fn read_frame(&mut self) -> Result<Frame, String> {
        let mut flags = [0u8; 1];
        self.read_exact(&mut flags)?;
        let size = if flags[0] & LONG != 0 {
            let mut b = [0u8; 8];
            self.read_exact(&mut b)?;
            u64::from_be_bytes(b)
        } else {
            let mut b = [0u8; 1];
            self.read_exact(&mut b)?;
            u64::from(b[0])
        };
        if size > 1 << 31 {
            return Err(format!("frame of {size} bytes is too large"));
        }
        let mut body = vec![0u8; size as usize];
        self.read_exact(&mut body)?;
        Ok(Frame {
            flags: flags[0],
            body,
        })
    }

    fn write_all(&mut self, b: &[u8]) -> Result<(), String> {
        self.stream.write_all(b).map_err(|e| e.to_string())
    }

    fn read_exact(&mut self, b: &mut [u8]) -> Result<(), String> {
        self.stream.read_exact(b).map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                "timed out waiting for the backend".to_owned()
            }
            _ => e.to_string(),
        })
    }
}

/// Appends a ZMTP metadata property.
fn property(out: &mut Vec<u8>, name: &str, value: &[u8]) {
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-connection REP peer for tests: answers each request with
    /// `reply(body)`.
    pub(crate) fn rep_server(reply: impl Fn(&[u8]) -> Vec<u8> + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut s = ReqSocket { stream };
            let mut g = [0u8; 64];
            s.read_exact(&mut g).unwrap();
            let mut greeting = [0u8; 64];
            greeting[0] = 0xFF;
            greeting[9] = 0x7F;
            greeting[10] = 3;
            greeting[12..16].copy_from_slice(b"NULL");
            greeting[32] = 1; // as-server
            s.write_all(&greeting).unwrap();
            let _ready = s.read_frame().unwrap();
            let mut ready = vec![5];
            ready.extend_from_slice(b"READY");
            property(&mut ready, "Socket-Type", b"REP");
            s.write_frame(COMMAND, &ready).unwrap();
            while let Ok(delim) = s.read_frame() {
                assert!(delim.body.is_empty() && delim.flags & MORE != 0);
                let msg = s.read_frame().unwrap();
                s.write_frame(MORE, &[]).unwrap();
                s.write_frame(0, &reply(&msg.body)).unwrap();
            }
        });
        format!("tcp://{addr}")
    }

    #[test]
    fn request_reply_round_trip_including_long_frames() {
        let ep = rep_server(|b| b.iter().rev().copied().collect());
        let mut s = ReqSocket::connect(&ep, Duration::from_secs(2)).unwrap();
        assert_eq!(s.request(b"abc").unwrap(), b"cba");
        let big: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        let back = s.request(&big).unwrap();
        assert_eq!(back.len(), big.len());
        assert_eq!(back[0], big[big.len() - 1]);
    }

    #[test]
    fn non_zmtp_peers_are_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.write_all(&[b'H'; 64]);
        });
        let e = ReqSocket::connect(&format!("tcp://{addr}"), Duration::from_secs(2)).unwrap_err();
        assert!(e.contains("not a ZMTP peer"), "{e}");
    }
}
