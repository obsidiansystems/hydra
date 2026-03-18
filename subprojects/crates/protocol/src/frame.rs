use bytes::{Buf, BufMut, BytesMut};
use serde::{Deserialize, Serialize};

/// Encode a message as a length-prefixed postcard frame: `[u32 LE length][postcard bytes]`.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    let payload = postcard::to_allocvec(msg)?;
    let len = u32::try_from(payload.len()).map_err(|_| postcard::Error::SerializeBufferFull)?;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Decode a single message from a length-prefixed postcard frame.
pub fn decode<'de, T: Deserialize<'de>>(data: &'de [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(data)
}

/// Incremental frame reader that buffers incoming bytes and yields complete messages.
pub struct FrameReader {
    buf: BytesMut,
}

impl FrameReader {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(8192),
        }
    }

    /// Append raw bytes from the transport.
    pub fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Try to extract the next complete frame. Returns the raw payload bytes (without length prefix).
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < 4 {
            return None;
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if self.buf.len() < 4 + len {
            return None;
        }
        self.buf.advance(4);
        let payload = self.buf.split_to(len).to_vec();
        Some(payload)
    }
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Frame writer that encodes messages into length-prefixed postcard frames.
pub struct FrameWriter {
    buf: Vec<u8>,
}

impl FrameWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(8192),
        }
    }

    /// Encode a message and return the frame bytes.
    pub fn encode<T: Serialize>(&mut self, msg: &T) -> Result<Vec<u8>, postcard::Error> {
        self.buf.clear();
        let payload = postcard::to_allocvec(msg)?;
        let len =
            u32::try_from(payload.len()).map_err(|_| postcard::Error::SerializeBufferFull)?;
        self.buf.put_u32_le(len);
        self.buf.extend_from_slice(&payload);
        Ok(self.buf.clone())
    }
}

impl Default for FrameWriter {
    fn default() -> Self {
        Self::new()
    }
}
