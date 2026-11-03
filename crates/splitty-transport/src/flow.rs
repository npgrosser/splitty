//! Retain byte allowance until both the writer and the recipient are done.
//! Receipt trailers are unpredictable and single use; stale generations cannot
//! acknowledge data belonging to a replacement connection.
use crate::{
    budget::Lease,
    queue::QUEUE_ITEMS,
    wire::{Credit, Delivery},
};
use std::{collections::HashMap, io, sync::Arc};

struct Pending {
    lease: Arc<Lease>,
    confirmed: Option<tokio::sync::oneshot::Sender<()>>,
}
pub struct Credits {
    generation: u32,
    pending: HashMap<[u8; 16], Pending>,
}
impl Credits {
    pub fn new(generation: u32) -> Self {
        assert_ne!(generation, 0);
        Self {
            generation,
            pending: HashMap::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    /// The writer retains the returned guard through all partial writes/IPC
    /// chunks. A racing acknowledgement cannot release a still-active buffer.
    pub fn track(&mut self, delivery: Delivery, lease: Lease) -> io::Result<Arc<Lease>> {
        self.track_confirmed(delivery, lease, None)
    }
    pub fn track_confirmed(
        &mut self,
        delivery: Delivery,
        lease: Lease,
        confirmed: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> io::Result<Arc<Lease>> {
        if delivery.generation != self.generation || self.pending.contains_key(&delivery.receipt) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid delivery receipt",
            ));
        }
        if self.pending.len() >= QUEUE_ITEMS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "delivery receipt allowance exhausted",
            ));
        }
        let lease = Arc::new(lease);
        self.pending.insert(
            delivery.receipt,
            Pending {
                lease: lease.clone(),
                confirmed,
            },
        );
        Ok(lease)
    }
    pub fn acknowledge(&mut self, credit: &Credit) -> io::Result<bool> {
        if credit.generation != self.generation {
            return Ok(false);
        }
        let receipt = unhex(&credit.receipt).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "malformed delivery receipt")
        })?;
        let pending = self.pending.remove(&receipt).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown or duplicate delivery receipt",
            )
        })?;
        drop(pending.lease);
        if let Some(confirmed) = pending.confirmed {
            let _ = confirmed.send(());
        }
        Ok(true)
    }
}

pub fn credit(delivery: Delivery) -> Credit {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut receipt = String::with_capacity(32);
    for byte in delivery.receipt {
        receipt.push(char::from(HEX[usize::from(byte >> 4)]));
        receipt.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Credit {
        generation: delivery.generation,
        receipt,
    }
}
fn unhex(text: &str) -> Option<[u8; 16]> {
    fn digit(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        }
    }
    if text.len() != 32 {
        return None;
    }
    let mut receipt = [0; 16];
    for (dest, pair) in receipt.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        *dest = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Some(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{ConnectionBudget, ProcessBudget};
    #[test]
    fn credits_cannot_free_unknown_stale_duplicate_or_still_writing_bytes() {
        let budget = ConnectionBudget::new(ProcessBudget::new(128, 16), 64, 8);
        let delivery = Delivery::new(4);
        let mut ledger = Credits::new(4);
        let writing = ledger
            .track(delivery, budget.reserve(40, false).unwrap())
            .unwrap();
        let mut stale = credit(delivery);
        stale.generation = 3;
        assert!(!ledger.acknowledge(&stale).unwrap());
        assert!(ledger.acknowledge(&credit(Delivery::new(4))).is_err());
        assert_eq!(budget.used(), 40);
        assert!(ledger.acknowledge(&credit(delivery)).unwrap());
        assert_eq!(
            budget.used(),
            40,
            "a racing credit cannot release the writer's buffer"
        );
        assert!(ledger.acknowledge(&credit(delivery)).is_err());
        drop(writing);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn writer_completion_alone_does_not_acknowledge_a_frozen_recipient() {
        let budget = ConnectionBudget::default();
        let mut ledger = Credits::new(9);
        drop(
            ledger
                .track(Delivery::new(9), budget.reserve(200, false).unwrap())
                .unwrap(),
        );
        assert_eq!(budget.used(), 200);
        drop(ledger);
        assert_eq!(budget.used(), 0);
    }
}
