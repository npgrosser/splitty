//! Request IDs never repeat within a service generation. Cancelled mutations
//! retain their slot until completion: cancellation cannot bypass admission by
//! leaving unlimited effects running in detached tasks.
use crate::wire::MAX_PENDING_REQUESTS;
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    Stale,
    InvalidId,
    Busy,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled {
    pub id: u32,
    pub uncertain: bool,
}
struct Pending {
    mutation: bool,
    answered: bool,
    deadline: Option<Instant>,
}
pub struct Requests {
    generation: u32,
    last_id: u32,
    pending: BTreeMap<u32, Pending>,
    cancelled_ahead: BTreeSet<u32>,
}

impl Requests {
    pub fn new(generation: u32) -> Self {
        assert_ne!(generation, 0);
        Self {
            generation,
            last_id: 0,
            pending: BTreeMap::new(),
            cancelled_ahead: BTreeSet::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    pub fn admit(
        &mut self,
        generation: u32,
        id: u32,
        mutation: bool,
        deadline: Option<Instant>,
    ) -> Result<(), Rejection> {
        if generation != self.generation {
            return Err(Rejection::Stale);
        }
        if id == 0 || id <= self.last_id {
            return Err(Rejection::InvalidId);
        }
        self.last_id = id;
        let cancelled = self.cancelled_ahead.remove(&id);
        self.cancelled_ahead.retain(|waiting| *waiting > id);
        if cancelled {
            return Err(Rejection::Cancelled);
        }
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(Rejection::Busy);
        }
        self.pending.insert(
            id,
            Pending {
                mutation,
                answered: false,
                deadline,
            },
        );
        Ok(())
    }
    pub fn cancel(&mut self, generation: u32, id: u32) -> Result<Option<Cancelled>, Rejection> {
        if generation != self.generation {
            return Ok(None);
        }
        // Reserved control delivery may overtake a queued API frame. Remember
        // at most one window of future cancellations before effect admission.
        if id > self.last_id {
            if self.cancelled_ahead.contains(&id) {
                return Ok(None);
            }
            if self.cancelled_ahead.len() >= MAX_PENDING_REQUESTS {
                return Err(Rejection::Busy);
            }
            self.cancelled_ahead.insert(id);
            return Ok(Some(Cancelled {
                id,
                uncertain: false,
            }));
        }
        let Some(pending) = self.pending.get_mut(&id) else {
            return Ok(None);
        };
        if pending.answered {
            return Ok(None);
        }
        pending.answered = true;
        pending.deadline = None;
        let uncertain = pending.mutation;
        // Even an abortable read retains admission until its task has actually
        // stopped. Otherwise rapid cancel/reopen could accumulate task state.
        Ok(Some(Cancelled { id, uncertain }))
    }
    /// Return true only when this completion still has a live reply recipient.
    pub fn finish(&mut self, generation: u32, id: u32) -> bool {
        generation == self.generation && self.pending.remove(&id).is_some_and(|p| !p.answered)
    }
    pub fn expire(&mut self, now: Instant) -> Vec<Cancelled> {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline.is_some_and(|d| d <= now))
            .map(|(&id, _)| id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.cancel(self.generation, id).expect("known pending ID"))
            .collect()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().filter_map(|p| p.deadline).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_mutations_cannot_flood_detached_work_or_get_a_late_reply() {
        let mut requests = Requests::new(7);
        for id in 1..=64 {
            requests.admit(7, id, true, None).unwrap();
            assert!(requests.cancel(7, id).unwrap().unwrap().uncertain);
        }
        assert_eq!(requests.admit(7, 65, true, None), Err(Rejection::Busy));
        assert_eq!(requests.len(), 64);
        assert!(!requests.finish(6, 1));
        assert_eq!(requests.len(), 64);
        assert!(!requests.finish(7, 1));
        assert_eq!(requests.len(), 63);
        assert_eq!(requests.admit(7, 1, false, None), Err(Rejection::InvalidId));
        requests.admit(7, 66, false, None).unwrap();
    }
    #[test]
    fn deadlines_and_cancellation_only_affect_their_generation() {
        let mut requests = Requests::new(3);
        let now = Instant::now();
        requests.admit(3, 1, false, Some(now)).unwrap();
        requests.admit(3, 2, true, Some(now)).unwrap();
        assert_eq!(requests.admit(2, 3, false, None), Err(Rejection::Stale));
        assert!(requests.cancel(2, 1).unwrap().is_none());
        assert_eq!(
            requests.expire(now),
            vec![
                Cancelled {
                    id: 1,
                    uncertain: false
                },
                Cancelled {
                    id: 2,
                    uncertain: true
                }
            ]
        );
        assert_eq!(requests.len(), 2);
        assert!(requests.next_deadline().is_none());
        assert!(!requests.finish(3, 1));
        assert!(!requests.finish(3, 2));
        assert!(requests.is_empty());
    }
    #[test]
    fn an_overtaking_cancel_blocks_admission_and_future_cancel_metadata_is_bounded() {
        let mut requests = Requests::new(4);
        for id in 1..=64 {
            assert!(!requests.cancel(4, id).unwrap().unwrap().uncertain);
        }
        assert_eq!(requests.cancel(4, 65), Err(Rejection::Busy));
        assert_eq!(requests.admit(4, 1, true, None), Err(Rejection::Cancelled));
        assert_eq!(
            requests.admit(4, 64, false, None),
            Err(Rejection::Cancelled)
        );
        assert!(requests.cancelled_ahead.is_empty());
        assert!(requests.is_empty());
        requests.admit(4, 65, false, None).unwrap();
    }
}
