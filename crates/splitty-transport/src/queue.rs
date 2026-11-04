//! Ordered nonblocking queues, bounded both by item count and shared byte
//! leases. Saturation closes only this connection; producers never wait for a
//! slow consumer or spawn a task to hide its backlog.
use crate::budget::{Charged, ConnectionBudget};
use std::io;
use tokio::sync::{mpsc, watch};

pub const QUEUE_ITEMS: usize = 1024;

#[derive(Clone, Debug)]
pub struct Context {
    pub budget: ConnectionBudget,
    closed: watch::Sender<bool>,
    parent: Option<std::sync::Arc<Context>>,
}
impl Default for Context {
    fn default() -> Self {
        Self::new(ConnectionBudget::default())
    }
}
impl Context {
    pub fn new(budget: ConnectionBudget) -> Self {
        Self {
            budget,
            closed: watch::channel(false).0,
            parent: None,
        }
    }
    /// A backend hop has independent failure state, with the owner's byte
    /// budget and synchronous rejection of work after owner revocation. Its
    /// owner must also forward the parent's close notification to `close` so
    /// already blocked IO wakes; closing this hop never closes the parent.
    pub fn child(&self) -> Self {
        Self {
            budget: self.budget.clone(),
            closed: watch::channel(false).0,
            parent: Some(std::sync::Arc::new(self.clone())),
        }
    }
    pub fn close(&self) {
        self.closed.send_replace(true);
    }
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_closed())
    }
    pub fn closing(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }
    pub fn channel<T>(&self, capacity: usize) -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Sender {
                tx,
                context: self.clone(),
            },
            Receiver {
                rx,
                context: self.clone(),
            },
        )
    }
}

pub struct Sender<T> {
    tx: mpsc::Sender<Charged<T>>,
    context: Context,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            context: self.context.clone(),
        }
    }
}
impl<T> Sender<T> {
    pub fn send(&self, bytes: usize, control: bool, make: impl FnOnce() -> T) -> io::Result<()> {
        self.try_build(bytes, control, || Ok(make()))
    }
    pub fn try_build(
        &self,
        bytes: usize,
        control: bool,
        make: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<()> {
        if self.context.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection closed",
            ));
        }
        let slot = self.tx.try_reserve().map_err(|error| {
            if matches!(error, mpsc::error::TrySendError::Full(_)) {
                self.context.close();
                io::Error::new(io::ErrorKind::WouldBlock, "connection queue exhausted")
            } else {
                // A normal detach drops one PTY subscription. Its next producer
                // send must forget that subscriber, not close the whole client.
                io::Error::new(io::ErrorKind::BrokenPipe, "receiver detached")
            }
        })?;
        let lease = self
            .context
            .budget
            .reserve(bytes.max(1), control)
            .map_err(|error| {
                self.context.close();
                io::Error::new(io::ErrorKind::OutOfMemory, error)
            })?;
        slot.send(lease.charge(make()?));
        Ok(())
    }
    pub fn is_closed(&self) -> bool {
        self.context.is_closed() || self.tx.is_closed()
    }
    /// Transfer an already counted reassembly buffer without releasing its
    /// lease or charging it twice. Cross-connection charges are never accepted.
    pub fn forward(&self, value: Charged<T>) -> io::Result<()> {
        if self.context.is_closed() || !value.belongs_to(&self.context.budget) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "invalid queue transfer",
            ));
        }
        self.tx.try_send(value).map_err(|error| {
            if matches!(error, mpsc::error::TrySendError::Full(_)) {
                self.context.close();
                io::Error::new(io::ErrorKind::WouldBlock, "connection queue exhausted")
            } else {
                io::Error::new(io::ErrorKind::BrokenPipe, "receiver detached")
            }
        })
    }
}

pub struct Receiver<T> {
    rx: mpsc::Receiver<Charged<T>>,
    context: Context,
}
impl<T> Receiver<T> {
    fn discard(&mut self) {
        self.rx.close();
        while self.rx.try_recv().is_ok() {}
    }
    pub async fn recv(&mut self) -> Option<Charged<T>> {
        if self.context.is_closed() {
            self.discard();
            return None;
        }
        let mut closing = self.context.closing();
        let value = tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => { self.discard(); None }
            value = self.rx.recv() => value,
        };
        // A parent may have closed while this receive was pending, before
        // the owner's watcher forwards its wakeup to the child's watch.
        if self.context.is_closed() {
            self.discard();
            None
        } else {
            value
        }
    }
    pub fn try_recv(&mut self) -> Result<Charged<T>, mpsc::error::TryRecvError> {
        if self.context.is_closed() {
            self.discard();
            return Err(mpsc::error::TryRecvError::Disconnected);
        }
        self.rx.try_recv()
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn child_context_inherits_revocation_without_closing_its_owner() {
        let owner = Context::default();
        let failed = owner.child();
        failed.close();
        assert!(
            !owner.is_closed(),
            "backend failure is not owner cancellation"
        );
        let child = owner.child();
        let (sender, mut receiver) = child.channel(1);
        sender.send(4, false, || 7).unwrap();
        assert_eq!(owner.budget.used(), 4);
        owner.close();
        assert!(child.is_closed());
        assert!(
            sender.send(1, false, || 8).is_err(),
            "no scheduling gap for new work"
        );
        assert!(receiver.recv().await.is_none());
        assert_eq!(owner.budget.used(), 0);
    }
    #[tokio::test]
    async fn pending_child_receive_discards_queued_input_after_parent_revocation() {
        use std::{future::Future, task::Poll};
        let owner = Context::default();
        let child = owner.child();
        let (sender, mut receiver) = child.channel(2);
        let mut receive = Box::pin(receiver.recv());
        std::future::poll_fn(|cx| {
            assert!(receive.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        sender.send(4, false, || 7).unwrap();
        sender.send(4, false, || 8).unwrap();
        owner.close();
        assert!(
            receive.await.is_none(),
            "already pending receives must recheck the parent"
        );
        assert_eq!(
            owner.budget.used(),
            0,
            "the selected value and remaining queue are discarded"
        );
    }
    use crate::budget::ProcessBudget;
    #[tokio::test]
    async fn slow_consumer_closes_independently_and_keeps_in_flight_bytes_charged() {
        let process = ProcessBudget::new(100, 10);
        let a = Context::new(ConnectionBudget::new(process.clone(), 60, 5));
        let b = Context::new(ConnectionBudget::new(process.clone(), 60, 5));
        let (tx, mut rx) = a.channel(2);
        let (other, mut good) = b.channel(2);
        tx.send(20, false, || vec![1; 20]).unwrap();
        let delivered = rx.recv().await.unwrap();
        assert_eq!(a.budget.used(), 20, "dequeue is not consumption");
        tx.send(20, false, || vec![2; 20]).unwrap();
        assert!(tx
            .send(20, false, || panic!("reject before allocation"))
            .is_err());
        assert!(a.is_closed());
        assert!(!b.is_closed());
        other.send(20, false, || vec![3; 20]).unwrap();
        assert_eq!(good.recv().await.unwrap().value, vec![3; 20]);
        assert!(rx.recv().await.is_none());
        drop(delivered);
        drop(rx);
        assert_eq!(process.used(), 0);
    }
    #[tokio::test]
    async fn empty_message_flood_is_also_bounded_without_waiters() {
        let context = Context::default();
        let (tx, mut rx) = context.channel(2);
        tx.send(0, false, || ()).unwrap();
        tx.send(0, false, || ()).unwrap();
        assert!(tx.send(0, false, || ()).is_err());
        assert!(rx.recv().await.is_none());
        drop(rx);
        assert_eq!(context.budget.used(), 0);
    }
    #[test]
    fn synchronous_drain_never_delivers_after_revocation() {
        let context = Context::default();
        let (tx, mut rx) = context.channel(2);
        tx.send(20, false, || "stale input").unwrap();
        context.close();
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        assert_eq!(context.budget.used(), 0);
    }
    #[test]
    fn detaching_one_subscription_does_not_close_other_queues_of_the_client() {
        let context = Context::default();
        let (old, detached) = context.channel::<u8>(2);
        let (active, mut receiver) = context.channel(2);
        drop(detached);
        assert_eq!(
            old.send(1, false, || 1).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(!context.is_closed());
        active.send(1, false, || 2).unwrap();
        assert_eq!(receiver.try_recv().unwrap().value, 2);
    }
}
