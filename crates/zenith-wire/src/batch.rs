//! High-performance packet batching and coalesced write buffer for Zenith.
//!
//! Reduces system call overhead under high concurrency by grouping outgoing
//! framed packets into a single socket write (up to 64 packets or 32 KiB per flush)
//! instead of executing individual write syscalls for every discrete packet.

use crate::{WireError, varint_len, write_varint};
use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Default maximum number of packets bundled into a single batch.
pub const DEFAULT_MAX_BATCH_PACKETS: usize = 64;

/// Default maximum payload size (32 KiB) before an automatic batch flush.
pub const DEFAULT_MAX_BATCH_BYTES: usize = 32 * 1024;

/// A contiguous buffer that accumulates framed Minecraft packets to be written
/// in a single vectorized or coalesced `write_all` call to the underlying transport.
#[derive(Debug)]
pub struct PacketBatchWriter {
    buffer: BytesMut,
    packet_count: usize,
    max_packets: usize,
    max_bytes: usize,
}

impl Default for PacketBatchWriter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BATCH_PACKETS, DEFAULT_MAX_BATCH_BYTES)
    }
}

impl PacketBatchWriter {
    /// Creates a new `PacketBatchWriter` with custom packet and byte capacity limits.
    pub fn new(max_packets: usize, max_bytes: usize) -> Self {
        Self {
            buffer: BytesMut::with_capacity(max_bytes),
            packet_count: 0,
            max_packets: max_packets.max(1),
            max_bytes: max_bytes.max(1024),
        }
    }

    /// Number of packets currently buffered in the batch.
    pub fn packet_count(&self) -> usize {
        self.packet_count
    }

    /// Number of bytes currently queued in the batch buffer.
    pub fn queued_bytes(&self) -> usize {
        self.buffer.len()
    }

    /// Returns `true` if the buffer contains no buffered packets.
    pub fn is_empty(&self) -> bool {
        self.packet_count == 0
    }

    /// Appends a packet frame `[Length: VarInt][PacketID: VarInt][Payload]` to the batch.
    ///
    /// Returns `true` if the batch has reached either the packet count threshold
    /// or the byte capacity threshold and is ready to be flushed immediately.
    pub fn push(&mut self, packet_id: i32, payload: &[u8]) -> bool {
        let id_len = varint_len(packet_id);
        let total_len = id_len + payload.len();

        let header_len = varint_len(total_len as i32) + id_len;
        self.buffer.reserve(header_len + payload.len());

        write_varint(total_len as i32, &mut self.buffer);
        write_varint(packet_id, &mut self.buffer);
        self.buffer.put_slice(payload);
        self.packet_count += 1;

        self.packet_count >= self.max_packets || self.buffer.len() >= self.max_bytes
    }

    /// Appends raw pre-framed bytes directly to the batch buffer.
    pub fn push_raw(&mut self, framed_bytes: &[u8]) -> bool {
        self.buffer.extend_from_slice(framed_bytes);
        self.packet_count += 1;
        self.packet_count >= self.max_packets || self.buffer.len() >= self.max_bytes
    }

    /// Flushes all queued packet bytes to the async writer in a single syscall.
    /// Clears the batch buffer upon completion.
    pub async fn flush<W: AsyncWrite + Unpin>(
        &mut self,
        writer: &mut W,
    ) -> Result<usize, WireError> {
        if self.buffer.is_empty() {
            return Ok(0);
        }

        let written = self.buffer.len();
        writer
            .write_all(&self.buffer)
            .await
            .map_err(|e| WireError::Io(e.to_string()))?;
        self.buffer.clear();
        self.packet_count = 0;
        Ok(written)
    }

    /// Clears the batch buffer without writing it (e.g. on disconnect or error).
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.packet_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_raw_packet;

    #[tokio::test]
    async fn test_batch_writer_single_flush_multiple_packets() {
        let mut batch = PacketBatchWriter::new(64, 32 * 1024);
        assert!(batch.is_empty());

        let p1 = b"Packet 1 data";
        let p2 = b"Packet 2 longer data with payload";
        let p3 = b"Packet 3 final piece";

        assert!(!batch.push(0x01, p1));
        assert!(!batch.push(0x02, p2));
        assert!(!batch.push(0x03, p3));

        assert_eq!(batch.packet_count(), 3);
        assert!(batch.queued_bytes() > p1.len() + p2.len() + p3.len());

        let mut output = Vec::new();
        let flushed = batch.flush(&mut output).await.expect("flush failed");
        assert_eq!(flushed, output.len());
        assert!(batch.is_empty());
        assert_eq!(batch.packet_count(), 0);

        // Read back from the stream to verify all 3 frames arrive intact byte-for-byte
        let mut cursor = &output[..];
        let (id1, r1) = read_raw_packet(&mut cursor, 1024).await.unwrap();
        assert_eq!(id1, 0x01);
        assert_eq!(&r1, p1);

        let (id2, r2) = read_raw_packet(&mut cursor, 1024).await.unwrap();
        assert_eq!(id2, 0x02);
        assert_eq!(&r2, p2);

        let (id3, r3) = read_raw_packet(&mut cursor, 1024).await.unwrap();
        assert_eq!(id3, 0x03);
        assert_eq!(&r3, p3);

        assert!(cursor.is_empty());
    }

    #[test]
    fn test_batch_writer_capacity_threshold() {
        let mut batch = PacketBatchWriter::new(3, 1024);
        assert!(!batch.push(0x10, b"one"));
        assert!(!batch.push(0x10, b"two"));
        // 3rd packet hits max_packets threshold
        assert!(batch.push(0x10, b"three"));
    }
}
