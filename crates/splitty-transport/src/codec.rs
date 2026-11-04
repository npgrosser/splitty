//! Byte framing with admission before allocation and one deadline for an entire
//! partial frame. EOF at a frame boundary is distinct from a truncated frame.
use crate::budget::{Charged, ConnectionBudget, Lease};
use crate::wire::{self, Delivery, Kind};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{timeout_at, Instant};

mod json;
pub use json::allocation_bound as json_allocation_bound;
pub use json::typed_allocation_bound as json_typed_allocation_bound;

/// Reserve JSON-owned strings, containers and decoder scratch before parsing.
/// Keep the lease until the decoded object (including async request arguments)
/// is dropped. The input buffer needs its own, independent lease.
pub fn reserve_json(bytes: &[u8], budget: &ConnectionBudget, control: bool) -> io::Result<Lease> {
    budget
        .reserve(json_allocation_bound(bytes)?, control)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))
}

/// Typed serde conversions additionally format peer-supplied values on error.
/// This reservation includes that temporary formatting allocation.
pub fn reserve_json_typed(
    bytes: &[u8],
    budget: &ConnectionBudget,
    control: bool,
) -> io::Result<Lease> {
    budget
        .reserve(json_typed_allocation_bound(bytes)?, control)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))
}

#[derive(Debug)]
pub struct Frame {
    pub kind: Kind,
    pub payload: Vec<u8>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

impl Frame {
    pub fn json<T: Serialize>(kind: Kind, value: &T) -> io::Result<Self> {
        if kind.is_data() {
            return Err(invalid("data frames require a delivery header"));
        }
        let payload = json_bytes(value, kind.payload_limit())?;
        Ok(Self { kind, payload })
    }

    /// Reserve serialized control storage before allocating it. The returned
    /// lease follows the frame through delayed/partial handshake writes.
    pub fn control<T: Serialize>(
        kind: Kind,
        value: &T,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<Self>> {
        if kind.is_data() {
            return Err(invalid("data frames require a delivery header"));
        }
        let bytes = json_size(value, kind.payload_limit())? + wire::HEADER_BYTES;
        let lease = budget
            .reserve(bytes, true)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        Ok(lease.charge(Self::json(kind, value)?))
    }

    pub fn data(kind: Kind, delivery: Delivery, payload: &[u8]) -> io::Result<Self> {
        if !kind.is_data() || delivery.generation == 0 {
            return Err(invalid("invalid application frame"));
        }
        let size = payload
            .len()
            .checked_add(wire::DELIVERY_BYTES)
            .ok_or_else(|| invalid("frame overflow"))?;
        if size > kind.payload_limit() {
            return Err(invalid("native frame exceeds limit"));
        }
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(&delivery.generation.to_be_bytes());
        bytes.extend_from_slice(payload);
        // The receipt is last, so consuming only the first IPC chunk never
        // reveals the credit needed to release the rest of this frame.
        bytes.extend_from_slice(&delivery.receipt);
        Ok(Self {
            kind,
            payload: bytes,
        })
    }

    pub fn data_json<T: Serialize>(kind: Kind, delivery: Delivery, value: &T) -> io::Result<Self> {
        if !kind.is_data() || delivery.generation == 0 {
            return Err(invalid("invalid application frame"));
        }
        let size = json_size(value, kind.payload_limit() - wire::DELIVERY_BYTES)?;
        let mut prefix = Vec::with_capacity(size + wire::DELIVERY_BYTES);
        prefix.extend_from_slice(&delivery.generation.to_be_bytes());
        let mut payload = json_prefixed(value, size + 4, prefix)?;
        payload.extend_from_slice(&delivery.receipt);
        Ok(Self { kind, payload })
    }

    pub fn application(&self) -> io::Result<(Delivery, &[u8])> {
        if !self.kind.is_data() {
            return Err(invalid("expected application frame"));
        }
        Delivery::decode(&self.payload).ok_or_else(|| invalid("invalid delivery header"))
    }

    pub fn decode<T: DeserializeOwned>(&self) -> io::Result<T> {
        let bytes = if self.kind.is_data() {
            self.application()?.1
        } else {
            &self.payload
        };
        serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))
    }

    pub fn decode_charged<T: DeserializeOwned>(
        &self,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<T>> {
        let bytes = if self.kind.is_data() {
            self.application()?.1
        } else {
            &self.payload
        };
        let lease = reserve_json_typed(bytes, budget, !self.kind.is_data())?;
        // Drop serde's potentially input-sized diagnostic while its formatting
        // reservation is still held; only a fixed diagnostic may escape it.
        let value =
            serde_json::from_slice(bytes).map_err(|_| invalid("invalid native JSON payload"))?;
        Ok(lease.charge(value))
    }

    /// Control schemas are prevalidated before conversion, avoiding the
    /// generic typed-error multiplier for valid maximum-sized diagnostics.
    pub fn decode_control<T: wire::ControlMessage>(
        &self,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<T>> {
        if self.kind != T::KIND
            && !matches!(
                (self.kind, T::KIND),
                (Kind::Accept, Kind::Hello) | (Kind::Pong, Kind::Ping)
            )
        {
            return Err(invalid("unexpected native control type"));
        }
        let size = json::control_allocation_bound(&self.payload)?;
        let lease = budget
            .reserve(size, true)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        let json::ControlValue(value) = serde_json::from_slice(&self.payload)
            .map_err(|_| invalid("invalid native JSON payload"))?;
        if !json::valid_control(self.kind, &value) {
            return Err(invalid("invalid native control fields"));
        }
        let value: T =
            serde_json::from_value(value).map_err(|_| invalid("invalid native control fields"))?;
        Ok(lease.shrink(value.retained_bytes()).charge(value))
    }

    pub fn size(&self) -> usize {
        wire::HEADER_BYTES + self.payload.len()
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.payload.len() > self.kind.payload_limit() {
            return Err(invalid("native frame exceeds limit"));
        }
        if self.kind.is_data() {
            self.application()?;
        }
        Ok(())
    }

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let size =
            u32::try_from(self.payload.len()).map_err(|_| invalid("frame length overflow"))?;
        let mut bytes = Vec::with_capacity(self.size());
        bytes.push(self.kind as u8);
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }
}

/// A structured diagnostic carried as the JSON string in Closed. Both the
/// inner string and the outer frame reserve their exact encoded sizes before
/// allocation. Callers may shorten oversized diagnostics, but budget refusal
/// must propagate without retrying or dropping structured error information.
pub fn closed_diagnostic<T: Serialize>(
    diagnostic: &T,
    retryable: bool,
    budget: &ConnectionBudget,
) -> io::Result<Charged<Frame>> {
    let size = json_size(diagnostic, wire::MAX_CONTROL_BYTES)?;
    let bytes = budget
        .reserve(size + std::mem::size_of::<wire::Closed>(), true)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    let message = String::from_utf8(json_bytes(diagnostic, size)?)
        .map_err(|_| invalid("invalid diagnostic encoding"))?;
    let closed = bytes.charge(wire::Closed {
        code: wire::CloseCode::BackendLost,
        retryable,
        message,
    });
    Frame::control(Kind::Closed, &closed.value, budget)
}

/// Serialize into an exactly sized buffer after measuring against the limit.
/// Callers delivering it must reserve that size before invoking this function.
pub fn json_bytes<T: Serialize>(value: &T, limit: usize) -> io::Result<Vec<u8>> {
    let size = json_size(value, limit)?;
    json_prefixed(value, size, Vec::with_capacity(size))
}

fn json_prefixed<T: Serialize>(value: &T, limit: usize, prefix: Vec<u8>) -> io::Result<Vec<u8>> {
    struct Limited {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(invalid("encoded JSON exceeds frame limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Limited {
        bytes: prefix,
        limit,
    };
    serde_json::to_writer(&mut out, value).map_err(|error| invalid(error.to_string()))?;
    Ok(out.bytes)
}

/// Measure without allocating so queue admission precedes JSON serialization.
pub fn json_size<T: Serialize>(value: &T, limit: usize) -> io::Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let next = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| invalid("encoded JSON exceeds frame limit"))?;
            self.bytes = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|error| invalid(error.to_string()))?;
    Ok(counter.bytes)
}

/// Counts the complete frame, including its header, until the returned charge
/// is dropped. The deadline starts with the first byte and never slides.
pub async fn read<R: AsyncRead + Unpin>(
    reader: &mut R,
    budget: &ConnectionBudget,
) -> io::Result<Option<Charged<Frame>>> {
    read_limited(reader, budget, wire::MAX_FRAME_BYTES).await
}

pub async fn read_limited<R: AsyncRead + Unpin>(
    reader: &mut R,
    budget: &ConnectionBudget,
    frame_limit: usize,
) -> io::Result<Option<Charged<Frame>>> {
    let mut header = [0; wire::HEADER_BYTES];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    let deadline = Instant::now() + wire::FRAME_TIMEOUT;
    let kind = Kind::try_from(header[0]).map_err(invalid)?;
    timeout_at(deadline, reader.read_exact(&mut header[1..]))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "partial native header timed out")
        })??;
    let length = usize::try_from(u32::from_be_bytes(
        header[1..].try_into().expect("fixed header length"),
    ))
    .map_err(|_| invalid("frame length overflow"))?;
    if length
        > kind
            .payload_limit()
            .min(frame_limit.saturating_sub(wire::HEADER_BYTES))
    {
        return Err(invalid("native frame exceeds limit"));
    }
    if kind.is_data() && length < wire::DELIVERY_BYTES {
        return Err(invalid("missing delivery header"));
    }
    let lease = budget
        .reserve(length + wire::HEADER_BYTES, !kind.is_data())
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    let mut payload = vec![0; length];
    timeout_at(deadline, reader.read_exact(&mut payload))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "partial native payload timed out")
        })??;
    let frame = Frame { kind, payload };
    frame.validate()?;
    Ok(Some(lease.charge(frame)))
}

/// The caller retains any charge while writing, including across partial
/// writes. Only one writer may own a given carrier's byte ordering.
pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    frame.validate()?;
    let size = u32::try_from(frame.payload.len()).map_err(|_| invalid("frame length overflow"))?;
    let mut header = [0; wire::HEADER_BYTES];
    header[0] = frame.kind as u8;
    header[1..].copy_from_slice(&size.to_be_bytes());
    writer.write_all(&header).await?;
    writer.write_all(&frame.payload).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::ProcessBudget;

    fn budget() -> ConnectionBudget {
        ConnectionBudget::new(ProcessBudget::new(2048, 128), 1024, 128)
    }

    #[tokio::test]
    async fn split_headers_coalesced_frames_and_binary_bytes_roundtrip() {
        let (mut writer, mut reader) = tokio::io::duplex(2);
        let delivery = Delivery {
            generation: 7,
            receipt: [3; 16],
        };
        let first = Frame::data(Kind::Binary, delivery, &[0, 255, 128, 0, 13, 10]).expect("binary");
        let second = Frame::data(Kind::Text, delivery, "界".as_bytes()).expect("UTF-8");
        let producer = tokio::spawn(async move {
            write(&mut writer, &first).await.expect("first");
            write(&mut writer, &second).await.expect("second");
        });
        let budget = budget();
        let first = read(&mut reader, &budget)
            .await
            .expect("read")
            .expect("first");
        assert_eq!(
            first.value.application().expect("delivery"),
            (delivery, &[0, 255, 128, 0, 13, 10][..])
        );
        let second = read(&mut reader, &budget)
            .await
            .expect("read")
            .expect("second");
        assert_eq!(
            second.value.application().expect("delivery").1,
            "界".as_bytes()
        );
        assert_eq!(budget.used(), first.value.size() + second.value.size());
        assert!(read(&mut reader, &budget).await.expect("EOF").is_none());
        producer.await.expect("producer");
        drop((first, second));
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn bad_lengths_kinds_and_truncation_allocate_no_payload() {
        let budget = budget();
        for bytes in [
            vec![255],
            vec![1, 255, 255, 255, 255],
            vec![0, 0, 0, 0, 0xff],
            vec![1, 0, 0, 0, 1],
            vec![0, 0],
        ] {
            assert!(
                read(&mut bytes.as_slice(), &budget).await.is_err(),
                "{bytes:?}"
            );
            assert_eq!(budget.used(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fragmented_payload_has_one_deadline_and_releases_its_charge() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        writer
            .write_all(&[1, 0, 0, 0, 100, 0, 0, 0, 1])
            .await
            .expect("partial data");
        let budget = budget();
        let task_budget = budget.clone();
        let task = tokio::spawn(async move { read(&mut reader, &task_budget).await });
        tokio::task::yield_now().await;
        assert_eq!(
            budget.used(),
            105,
            "reassembly counts before the remaining payload arrives"
        );
        tokio::time::advance(Duration::from_secs(59)).await;
        writer
            .write_all(&[2])
            .await
            .expect("one more byte does not restart the deadline");
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            task.await.expect("reader").expect_err("deadline").kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn admission_precedes_reassembly_allocation() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer
            .write_all(&[1, 0, 0, 4, 0])
            .await
            .expect("length without a payload");
        let budget = budget();
        assert_eq!(
            read(&mut reader, &budget)
                .await
                .expect_err("no capacity")
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.data.high_water(), 0);
    }

    #[test]
    fn exhausted_controls_refuse_before_serialization_and_release_nested_scratch() {
        use crate::budget::ProcessBudget;
        use std::cell::Cell;
        struct Probe<'a>(&'a Cell<usize>);
        impl Serialize for Probe<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.set(self.0.get() + 1);
                serializer.serialize_str("diagnostic")
            }
        }
        let budget = ConnectionBudget::new(ProcessBudget::new(1024, 1024), 1024, 1024);
        let held = budget.reserve(1024, true).unwrap();
        let calls = Cell::new(0);
        assert_eq!(
            closed_diagnostic(&Probe(&calls), false, &budget)
                .unwrap_err()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(
            calls.get(),
            1,
            "only size measurement, no payload allocation/serialization"
        );
        calls.set(0);
        assert_eq!(
            Frame::control(Kind::Closed, &Probe(&calls), &budget)
                .unwrap_err()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(budget.used(), 1024);
        drop(held);
        let diagnostic = "x".repeat(400);
        let inner = json_size(&diagnostic, wire::MAX_CONTROL_BYTES).unwrap()
            + std::mem::size_of::<wire::Closed>();
        let held = budget.reserve(1024 - inner, true).unwrap();
        assert_eq!(
            closed_diagnostic(&diagnostic, true, &budget)
                .unwrap_err()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(
            budget.used(),
            1024 - inner,
            "failed outer admission releases the inner string"
        );
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    use std::time::Duration;
}
