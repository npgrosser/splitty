//! Client transactions for a dedicated API stream. Session streams must never use
//! this receiver: non-reply frames are consumed here, retaining their leases only
//! until inspected. The caller validates operation authority before submission.
use crate::{
    budget::Charged,
    peer::{Peer, Sender},
    wire::{self, Kind},
};
use std::{io, time::Duration};
#[derive(Default)]
pub struct RequestState {
    last: u32,
}
impl RequestState {
    pub async fn request(
        &mut self,
        peer: &mut Peer,
        operation: String,
        args: serde_json::Map<String, serde_json::Value>,
        duration: Option<Duration>,
    ) -> io::Result<Charged<wire::Reply>> {
        self.last = self
            .last
            .checked_add(1)
            .ok_or_else(|| io::Error::other("request generation exhausted"))?;
        let id = self.last;
        let sender = peer.sender.clone();
        sender.json(
            Kind::Request,
            &wire::Request {
                id,
                operation,
                args,
            },
        )?;
        struct Cancel {
            sender: Sender,
            id: Option<u32>,
        }
        impl Drop for Cancel {
            fn drop(&mut self) {
                if let Some(id) = self.id {
                    let _ = self.sender.control(
                        Kind::Cancel,
                        &wire::Cancel {
                            generation: self.sender.generation(),
                            id,
                        },
                    );
                }
            }
        }
        let mut cancel = Cancel {
            sender,
            id: Some(id),
        };
        let wait = async {
            while let Some(message) = peer.recv().await {
                if message.frame.kind != Kind::Reply {
                    continue;
                }
                let reply = message
                    .frame
                    .decode_charged::<wire::Reply>(&cancel.sender.context().budget)?;
                if reply.id < id {
                    continue;
                } // cancelled older request, never reused
                if reply.id != id {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "reply does not match native request",
                    ));
                }
                return Ok(reply);
            }
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "native connection lost; an admitted mutation may have completed",
            ))
        };
        let result = if let Some(duration) = duration {
            tokio::time::timeout(duration, wait).await.map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "native request deadline exceeded; an admitted mutation may have completed",
                )
            })?
        } else {
            wait.await
        };
        if result.is_ok() {
            cancel.id = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{peer::Incoming, queue};
    fn peers() -> (Peer, Peer) {
        let ready = wire::Ready {
            generation: 3,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let (a, b) = tokio::io::duplex(65536);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (
            Peer::start(ar, aw, &ready, queue::Context::default()),
            Peer::start(br, bw, &ready, queue::Context::default()),
        )
    }
    fn request(message: &Incoming) -> wire::Request {
        assert_eq!(message.frame.kind, Kind::Request);
        message.frame.decode().unwrap()
    }
    fn reply(peer: &Peer, id: u32) {
        peer.sender
            .json(
                Kind::Reply,
                &wire::Reply {
                    id,
                    status: 200,
                    content_type: wire::ContentType::Json,
                    body: "{\"ok\":true}".into(),
                    uncertain: false,
                },
            )
            .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_request_is_not_replayed_and_late_reply_cannot_settle_next() {
        let (mut client, mut server) = peers();
        let worker = tokio::spawn(async move {
            let first = request(&server.recv().await.unwrap());
            let cancel = server.recv().await.unwrap();
            assert_eq!(cancel.frame.kind, Kind::Cancel);
            let cancel: wire::Cancel = serde_json::from_slice(&cancel.frame.payload).unwrap();
            assert_eq!(
                cancel,
                wire::Cancel {
                    generation: 3,
                    id: first.id
                }
            );
            let second = request(&server.recv().await.unwrap());
            assert_eq!(second.id, first.id + 1);
            reply(&server, first.id);
            reply(&server, second.id);
            while server.recv().await.is_some() {}
            server.finish().await.unwrap();
        });
        let mut state = RequestState::default();
        let error = state
            .request(
                &mut client,
                "home.update".into(),
                Default::default(),
                Some(Duration::from_millis(10)),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("may have completed"));
        let budget = client.sender.context().budget.clone();
        let response = state
            .request(
                &mut client,
                "home.get".into(),
                Default::default(),
                Some(Duration::from_secs(1)),
            )
            .await
            .unwrap();
        assert_eq!(response.id, 2);
        assert!(budget.used() > 0, "decoded response remains charged");
        drop(response);
        client.finish().await.unwrap();
        worker.await.unwrap();
        assert_eq!(budget.used(), 0);
    }
    #[tokio::test]
    async fn future_reply_is_rejected_and_lost_mutation_remains_uncertain() {
        for future_reply in [true, false] {
            let (mut client, mut server) = peers();
            let worker = tokio::spawn(async move {
                let incoming = request(&server.recv().await.unwrap());
                if future_reply {
                    reply(&server, incoming.id + 1);
                    while server.recv().await.is_some() {}
                }
                server.finish().await.unwrap();
            });
            let error = RequestState::default()
                .request(
                    &mut client,
                    "home.update".into(),
                    Default::default(),
                    Some(Duration::from_secs(1)),
                )
                .await
                .unwrap_err();
            assert_eq!(
                error.kind(),
                if future_reply {
                    io::ErrorKind::InvalidData
                } else {
                    io::ErrorKind::ConnectionAborted
                }
            );
            if !future_reply {
                assert!(error.to_string().contains("may have completed"));
            }
            client.finish().await.unwrap();
            worker.await.unwrap();
        }
    }
}
