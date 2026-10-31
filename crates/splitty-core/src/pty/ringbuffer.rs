//! Thread-safe circular byte buffer for caching PTY scrollback output.
//!
//! Oldest data is silently overwritten when the buffer is full. Reads
//! always return data in chronological order (oldest to newest).

use std::sync::Mutex;

/// A thread-safe circular byte buffer for caching PTY output.
pub struct RingBuffer {
    inner: Mutex<Inner>,
}

struct Inner {
    buf: Vec<u8>,
    w: usize,   // next write position
    len: usize, // current data length, capped at capacity
    total: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum Selection {
    From(u64),
    Oldest,
    Tail(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub struct BufferSlice {
    pub data: Vec<u8>,
    pub oldest: u64,
    pub end: u64,
    pub next: u64,
    pub lost: u64,
}

impl Inner {
    fn oldest(&self) -> u64 {
        self.total - u64::try_from(self.len).expect("retained output fits a 64-bit cursor")
    }

    fn byte(&self, position: u64) -> u8 {
        let offset =
            usize::try_from(position - self.oldest()).expect("position is inside the ring");
        self.buf[(self.w + self.buf.len() - self.len + offset) % self.buf.len()]
    }

    fn boundary(&self, position: u64) -> bool {
        position == self.total || self.byte(position) & 0xc0 != 0x80
    }
}

impl RingBuffer {
    /// Creates a ring buffer with the given capacity in bytes.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "a scrollback ring needs positive capacity");
        Self {
            inner: Mutex::new(Inner {
                buf: vec![0u8; capacity],
                w: 0,
                len: 0,
                total: 0,
            }),
        }
    }

    /// Appends data to the buffer, overwriting oldest bytes if full.
    pub fn write(&self, data: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        let capacity = inner.buf.len();
        inner.total = inner
            .total
            .checked_add(u64::try_from(data.len()).expect("output length fits a cursor"))
            .expect("output cursor exhausted after 16 exabytes");

        if data.len() >= capacity {
            // Data larger than buffer -- keep the last `capacity` bytes.
            inner.buf.copy_from_slice(&data[data.len() - capacity..]);
            inner.w = 0;
            inner.len = capacity;
            return;
        }

        // Copy data starting at w, wrapping around if needed.
        let w = inner.w;
        let first = (capacity - w).min(data.len());
        inner.buf[w..w + first].copy_from_slice(&data[..first]);
        if first < data.len() {
            inner.buf[..data.len() - first].copy_from_slice(&data[first..]);
        }
        inner.w = (inner.w + data.len()) % capacity;
        inner.len = (inner.len + data.len()).min(capacity);
    }

    /// Returns a copy of all buffered data from oldest to newest.
    pub fn read_all(&self) -> Vec<u8> {
        let inner = self.inner.lock().unwrap();
        if inner.len == 0 {
            return Vec::new();
        }

        let capacity = inner.buf.len();
        let mut out = vec![0u8; inner.len];

        if inner.len < capacity {
            // Buffer not full -- data starts at 0.
            out.copy_from_slice(&inner.buf[..inner.len]);
        } else {
            // Buffer full -- oldest data starts at w.
            let first = capacity - inner.w;
            out[..first].copy_from_slice(&inner.buf[inner.w..]);
            out[first..].copy_from_slice(&inner.buf[..inner.w]);
        }
        out
    }

    /// Take one bounded snapshot without consuming any reader's position.
    /// Text rings contain valid UTF-8; a wrapped leading fragment is reported lost.
    pub fn read(
        &self,
        selection: Selection,
        limit: usize,
        text: bool,
    ) -> Result<BufferSlice, &'static str> {
        if limit == 0 || (text && limit < 4) {
            return Err("invalid output limit");
        }
        let inner = self.inner.lock().expect("output ring lock");
        let mut oldest = inner.oldest();
        if text {
            while oldest < inner.total && !inner.boundary(oldest) {
                oldest += 1;
            }
        }
        let requested = match selection {
            Selection::From(position) => position,
            Selection::Oldest => inner.oldest(),
            Selection::Tail(lines) => {
                if !text {
                    return Err("tail requires text output");
                }
                let mut start = inner.total;
                let mut remaining = lines;
                if remaining > 0 && start > oldest && inner.byte(start - 1) == b'\n' {
                    start -= 1;
                }
                while remaining > 0 && start > oldest {
                    start -= 1;
                    if inner.byte(start) == b'\n' {
                        remaining -= 1;
                        if remaining == 0 {
                            start += 1;
                        }
                    }
                }
                let cap = u64::try_from(limit).expect("output limit fits a cursor");
                let mut start = start.max(inner.total.saturating_sub(cap));
                while start < inner.total && !inner.boundary(start) {
                    start += 1;
                }
                start
            }
        };
        if requested > inner.total {
            return Err("cursor is beyond captured output");
        }
        let start = requested.max(oldest);
        if text && !inner.boundary(start) {
            return Err("cursor is not a text boundary");
        }
        let mut end = start
            .saturating_add(u64::try_from(limit).expect("output limit fits a cursor"))
            .min(inner.total);
        if text {
            while end > start && !inner.boundary(end) {
                end -= 1;
            }
        }
        let data = (start..end).map(|position| inner.byte(position)).collect();
        Ok(BufferSlice {
            data,
            oldest,
            end: inner.total,
            next: end,
            lost: oldest.saturating_sub(requested),
        })
    }

    /// Returns the current data length.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len
    }

    /// Returns true if the buffer contains no data.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().len == 0
    }

    /// Returns the buffer capacity.
    pub fn capacity(&self) -> usize {
        self.inner.lock().unwrap().buf.len()
    }

    /// Returns (used, capacity).
    pub fn stats(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.len, inner.buf.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let rb = RingBuffer::new(16);
        rb.write(b"hello");
        assert_eq!(rb.read_all(), b"hello");
        assert_eq!(rb.len(), 5);
    }

    #[test]
    fn test_write_wrapping() {
        let rb = RingBuffer::new(8);
        rb.write(b"abcdef"); // 6 bytes, w=6
        rb.write(b"ghij"); // 4 bytes, wraps: w=(6+4)%8=2
                           // Buffer full: oldest at w=2: "ijabcdef" -> no, let's think:
                           // After first write: buf=[a,b,c,d,e,f,0,0], w=6, len=6
                           // Second write: first copy 2 bytes at [6..8]="gh", remaining 2 bytes at [0..2]="ij"
                           // w=2, len=min(10,8)=8
                           // read_all: buf full, oldest at w=2: buf[2..8]+buf[0..2] = "cdefgh" + "ij" = "cdefghij"
        assert_eq!(rb.read_all(), b"cdefghij");
    }

    #[test]
    fn test_overflow() {
        let rb = RingBuffer::new(4);
        rb.write(b"ab");
        rb.write(b"cd");
        rb.write(b"ef");
        // After "ab": buf=[a,b,0,0], w=2, len=2
        // After "cd": buf=[a,b,c,d], w=0, len=4
        // After "ef": buf=[e,f,c,d], w=2, len=4
        // read_all: full, oldest at w=2: "cd"+"ef" = "cdef"
        assert_eq!(rb.read_all(), b"cdef");
    }

    #[test]
    fn test_data_larger_than_capacity() {
        let rb = RingBuffer::new(4);
        rb.write(b"abcdefgh");
        // Keeps last 4 bytes
        assert_eq!(rb.read_all(), b"efgh");
        assert_eq!(rb.len(), 4);
    }

    #[test]
    fn test_read_all_empty() {
        let rb = RingBuffer::new(8);
        assert_eq!(rb.read_all(), Vec::<u8>::new());
    }

    #[test]
    fn test_multiple_writes() {
        let rb = RingBuffer::new(16);
        rb.write(b"hello");
        rb.write(b" ");
        rb.write(b"world");
        assert_eq!(rb.read_all(), b"hello world");
        assert_eq!(rb.len(), 11);
    }

    #[test]
    fn cursor_pages_are_independent_and_report_overwrite() {
        let ring = RingBuffer::new(8);
        ring.write(b"abcdef");
        let first = ring.read(Selection::From(0), 3, false).expect("page");
        assert_eq!(first.data, b"abc");
        assert_eq!(first.next, 3);
        assert_eq!(
            ring.read(Selection::From(0), 3, false)
                .expect("other reader"),
            first
        );
        ring.write(b"ghijkl");
        let next = ring
            .read(Selection::From(first.next), 3, false)
            .expect("wrapped page");
        assert_eq!(next.oldest, 4);
        assert_eq!(next.lost, 1);
        assert_eq!(next.data, b"efg");
        assert_eq!(next.next, 7);
        assert_eq!(next.end, 12);
        assert!(ring.read(Selection::From(13), 1, false).is_err());
    }

    #[test]
    fn text_tail_and_pages_stay_on_character_boundaries() {
        let ring = RingBuffer::new(12);
        ring.write("old\nö🙂\nend\n".as_bytes());
        let tail = ring.read(Selection::Tail(1), 64, true).expect("tail");
        assert_eq!(tail.data, b"end\n");
        let end = ring.read(Selection::Tail(0), 4, true).expect("tail zero");
        assert!(end.data.is_empty());
        assert_eq!(end.next, end.end);
        let mut cursor = 0;
        let mut data = Vec::new();
        loop {
            let slice = ring
                .read(Selection::From(cursor), 4, true)
                .expect("text page");
            assert!(std::str::from_utf8(&slice.data).is_ok());
            data.extend(&slice.data);
            cursor = slice.next;
            if cursor == slice.end {
                break;
            }
            assert!(!slice.data.is_empty());
        }
        assert!(std::str::from_utf8(&data)
            .expect("text")
            .ends_with("ö🙂\nend\n"));
        let narrow = RingBuffer::new(3);
        narrow.write("🙂x".as_bytes());
        let read = narrow
            .read(Selection::From(0), 4, true)
            .expect("wrapped scalar");
        assert_eq!(read.data, b"x");
        assert_eq!(read.lost, 4);
    }

    #[test]
    fn range_snapshots_remain_coherent_while_writing() {
        let ring = std::sync::Arc::new(RingBuffer::new(257));
        let writer = ring.clone();
        let task = std::thread::spawn(move || {
            for i in 0..10000_u64 {
                writer.write(&[u8::try_from(i % 251).expect("byte")]);
            }
        });
        let mut cursor = 0;
        while cursor < 10000 {
            let slice = ring
                .read(Selection::From(cursor), 31, false)
                .expect("snapshot");
            let start = cursor.max(slice.oldest);
            for (offset, byte) in slice.data.iter().enumerate() {
                assert_eq!(
                    u64::from(*byte),
                    (start + u64::try_from(offset).expect("offset")) % 251
                );
            }
            assert_eq!(
                slice.next,
                start + u64::try_from(slice.data.len()).expect("len")
            );
            cursor = slice.next;
            std::thread::yield_now();
        }
        task.join().expect("writer");
    }
}
