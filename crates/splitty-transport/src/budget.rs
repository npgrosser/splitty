//! Count queued, reassembled and in-flight wire bytes until delivery completes.
//! Data cannot consume the capacity reserved for liveness/cancellation/credits.
use crate::wire;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
#[error("transport byte budget exhausted")]
pub struct Exhausted;

#[derive(Debug)]
pub struct Budget {
    limit: usize,
    used: AtomicUsize,
    high_water: AtomicUsize,
}

impl Budget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: AtomicUsize::new(0),
            high_water: AtomicUsize::new(0),
        })
    }
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    pub fn high_water(&self) -> usize {
        self.high_water.load(Ordering::Acquire)
    }
    pub fn acquire(self: &Arc<Self>, bytes: usize) -> Result<Permit, Exhausted> {
        let mut used = self.used.load(Ordering::Acquire);
        let reserved = loop {
            let next = used
                .checked_add(bytes)
                .filter(|next| *next <= self.limit)
                .ok_or(Exhausted)?;
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break next,
                Err(current) => used = current,
            }
        };
        self.high_water.fetch_max(reserved, Ordering::AcqRel);
        Ok(Permit {
            budget: self.clone(),
            bytes,
        })
    }
}

#[derive(Debug)]
pub struct Permit {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub struct ProcessBudget {
    pub data: Arc<Budget>,
    pub control: Arc<Budget>,
}
impl ProcessBudget {
    pub fn global() -> Arc<Self> {
        static PROCESS: OnceLock<Arc<ProcessBudget>> = OnceLock::new();
        PROCESS
            .get_or_init(|| Self::new(wire::PROCESS_BYTES, wire::PROCESS_CONTROL_BYTES))
            .clone()
    }
    pub fn new(total: usize, control: usize) -> Arc<Self> {
        assert!(
            control <= total,
            "control reservation exceeds process budget"
        );
        Arc::new(Self {
            data: Budget::new(total - control),
            control: Budget::new(control),
        })
    }
    pub fn used(&self) -> usize {
        self.data.used() + self.control.used()
    }
}

#[derive(Clone, Debug)]
pub struct ConnectionBudget {
    pub data: Arc<Budget>,
    pub control: Arc<Budget>,
    pub process: Arc<ProcessBudget>,
}

#[derive(Debug)]
pub struct Lease {
    _local: Permit,
    _process: Permit,
}

#[derive(Debug)]
pub struct Charged<T> {
    pub value: T,
    lease: Lease,
}
impl<T> Charged<T> {
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Charged<U> {
        Charged {
            value: map(self.value),
            lease: self.lease,
        }
    }
    pub fn into_parts(self) -> (T, Lease) {
        (self.value, self.lease)
    }
    pub(crate) fn belongs_to(&self, budget: &ConnectionBudget) -> bool {
        Arc::ptr_eq(&self.lease._local.budget, &budget.data)
            || Arc::ptr_eq(&self.lease._local.budget, &budget.control)
    }
}

impl<T> std::ops::Deref for Charged<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Charged<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: Default> Default for Charged<T> {
    fn default() -> Self {
        ConnectionBudget::default()
            .charge(0, false, T::default)
            .expect("zero byte lease")
    }
}

impl<T: PartialEq<U>, U> PartialEq<U> for Charged<T> {
    fn eq(&self, other: &U) -> bool {
        self.value.eq(other)
    }
}

impl Default for ConnectionBudget {
    fn default() -> Self {
        Self::new(
            ProcessBudget::global(),
            wire::CONNECTION_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        )
    }
}

impl ConnectionBudget {
    pub fn new(process: Arc<ProcessBudget>, total: usize, control: usize) -> Self {
        assert!(
            control <= total,
            "control reservation exceeds connection budget"
        );
        Self {
            data: Budget::new(total - control),
            control: Budget::new(control),
            process,
        }
    }
    pub fn used(&self) -> usize {
        self.data.used() + self.control.used()
    }
    pub fn reserve(&self, bytes: usize, control: bool) -> Result<Lease, Exhausted> {
        let (local, process) = if control {
            (&self.control, &self.process.control)
        } else {
            (&self.data, &self.process.data)
        };
        Ok(Lease {
            _local: local.acquire(bytes)?,
            _process: process.acquire(bytes)?,
        })
    }
    /// Admission precedes allocation in `make`. The permit follows the buffer
    /// through queueing and framing until delivery/acknowledgement or teardown.
    pub fn charge<T>(
        &self,
        bytes: usize,
        control: bool,
        make: impl FnOnce() -> T,
    ) -> Result<Charged<T>, Exhausted> {
        let lease = self.reserve(bytes, control)?;
        Ok(Charged {
            value: make(),
            lease,
        })
    }
}

impl Lease {
    /// Release parser scratch after conversion while retaining the allocation
    /// that leaves the decoder. No new admission or allocation is involved.
    pub fn shrink(mut self, bytes: usize) -> Self {
        for permit in [&mut self._local, &mut self._process] {
            assert!(
                bytes <= permit.bytes,
                "a lease cannot grow without admission"
            );
            permit
                .budget
                .used
                .fetch_sub(permit.bytes - bytes, Ordering::AcqRel);
            permit.bytes = bytes;
        }
        self
    }
    pub fn charge<T>(self, value: T) -> Charged<T> {
        Charged { value, lease: self }
    }
}

/// Incremental response serialization reserves vector capacity before growing
/// it. Retain the returned leases through adapter encoding and delivery.
pub struct Buffer {
    bytes: Vec<u8>,
    leases: Vec<Lease>,
    budget: ConnectionBudget,
    limit: usize,
}
impl Buffer {
    pub fn new(budget: ConnectionBudget, limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            leases: Vec::new(),
            budget,
            limit,
        }
    }
    pub fn finish(self) -> (Vec<u8>, Vec<Lease>) {
        (self.bytes, self.leases)
    }
}
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "response exceeds native frame limit",
                )
            })?;
        if length > self.bytes.capacity() {
            let capacity = length
                .max(self.bytes.capacity().saturating_mul(2))
                .max(1024)
                .min(self.limit);
            let lease = self
                .budget
                .reserve(capacity - self.bytes.capacity(), false)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            self.bytes.reserve_exact(capacity - self.bytes.len());
            self.leases.push(lease);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Measure stable diagnostic formatting without allocating its output.
pub fn display_size(value: &impl std::fmt::Display) -> std::io::Result<usize> {
    struct Count(usize);
    impl std::fmt::Write for Count {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            self.0 = self.0.checked_add(value.len()).ok_or(std::fmt::Error)?;
            Ok(())
        }
    }
    let mut count = Count(0);
    std::fmt::write(&mut count, format_args!("{value}"))
        .map_err(|_| std::io::Error::other("diagnostic formatting exceeds capacity"))?;
    Ok(count.0)
}
impl ConnectionBudget {
    /// Reserve stable native formatting before building the owned string. The
    /// fixed allowance also covers moving it into a small typed event/error.
    pub fn display(
        &self,
        value: &impl std::fmt::Display,
        control: bool,
    ) -> std::io::Result<Charged<String>> {
        let size = display_size(value)?;
        let lease = self
            .reserve(
                size.checked_add(256)
                    .ok_or_else(|| std::io::Error::other("diagnostic exceeds capacity"))?,
                control,
            )
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::WouldBlock, error))?;
        let mut text = String::with_capacity(size);
        std::fmt::write(&mut text, format_args!("{value}"))
            .map_err(|_| std::io::Error::other("diagnostic formatting failed"))?;
        Ok(lease.charge(text))
    }
}
impl<T: serde::Serialize> serde::Serialize for Charged<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

impl<T> std::borrow::Borrow<T> for Charged<T> {
    fn borrow(&self) -> &T {
        &self.value
    }
}
impl<T> std::borrow::BorrowMut<T> for Charged<T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn many_connections_share_capacity_and_reserve_control_delivery() {
        let process = ProcessBudget::new(100, 20);
        let a = ConnectionBudget::new(process.clone(), 90, 10);
        let b = ConnectionBudget::new(process.clone(), 90, 10);
        let held = a.reserve(80, false).expect("data window");
        assert!(b.reserve(1, false).is_err());
        assert_eq!(
            b.data.used(),
            0,
            "failed process acquisition rolls back local charge"
        );
        let control = b.reserve(10, true).expect("cancel/liveness capacity");
        assert_eq!(process.used(), 90);
        drop(held);
        assert_eq!(process.used(), 10);
        drop(control);
        assert_eq!(process.used(), 0);
    }
    #[test]
    fn rejection_happens_before_allocating_and_mapping_keeps_the_charge() {
        let connection = ConnectionBudget::new(ProcessBudget::new(64, 4), 32, 4);
        assert!(connection
            .charge(29, false, || panic!("must not allocate"))
            .is_err());
        let charged = connection
            .charge(20, false, || vec![0; 20])
            .expect("allocation");
        let mapped = charged.map(|bytes| String::from_utf8(bytes).expect("UTF-8"));
        assert_eq!(connection.used(), 20);
        drop(mapped);
        assert_eq!(connection.used(), 0);
        assert_eq!(connection.data.high_water(), 20);
    }
    #[test]
    fn sixteen_connections_cannot_exceed_aggregate_bytes_even_with_control_reservations() {
        let process = ProcessBudget::new(128, 8);
        let connections: Vec<_> = (0..16)
            .map(|_| ConnectionBudget::new(process.clone(), 32, 4))
            .collect();
        let held: Vec<_> = connections
            .iter()
            .filter_map(|c| c.reserve(20, false).ok())
            .collect();
        assert_eq!(held.len(), 6);
        assert_eq!(process.data.high_water(), 120);
        let controls: Vec<_> = connections
            .iter()
            .filter_map(|c| c.reserve(1, true).ok())
            .collect();
        assert_eq!(controls.len(), 8);
        assert_eq!(process.used(), 128);
        for c in &connections {
            assert!(c.reserve(1, false).is_err());
            assert!(c.used() <= 32);
        }
        drop(held);
        drop(controls);
        assert_eq!(process.used(), 0);
    }
}
