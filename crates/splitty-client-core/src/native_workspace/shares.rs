//! The local service's reverse forwards for the window. Its stream is opened
//! when the user first looks at the shares of a host or shares a port, so a
//! window that never shares holds no stream for it.
use super::HubSetup;
use crate::hub::{ShareRequest, SharesClient, SharesSnapshot};
use std::sync::Arc;

pub(super) struct RuntimeShares<'a> {
    service: &'a crate::owned_service::OwnedService,
    setup: Option<HubSetup<'a>>,
    setup_context: Option<splitty_transport::queue::Context>,
    client: Option<SharesClient>,
    /// Requests made while the stream was being opened.
    backlog: Vec<ShareRequest>,
    failed: bool,
}
impl<'a> RuntimeShares<'a> {
    pub(super) fn new(service: &'a crate::owned_service::OwnedService) -> Self {
        Self {
            service,
            setup: None,
            setup_context: None,
            client: None,
            backlog: Vec::new(),
            failed: false,
        }
    }
    fn open(&mut self) {
        if self.client.is_none() && self.setup.is_none() && !self.failed {
            let context = splitty_transport::queue::Context::default();
            self.setup = Some(Box::pin(
                self.service.api_stream_with_context(context.clone()),
            ));
            self.setup_context = Some(context);
        }
    }
    pub(super) async fn next_update(&mut self) -> Arc<SharesSnapshot> {
        loop {
            if let Some(client) = &mut self.client {
                return client.next_update().await;
            }
            let Some(setup) = &mut self.setup else {
                return std::future::pending().await;
            };
            let result = setup.await;
            self.setup = None;
            self.setup_context = None;
            match result {
                Ok(stream) => {
                    let mut client = SharesClient::new(stream);
                    for request in self.backlog.drain(..) {
                        let _ = client.submit(request);
                    }
                    self.client = Some(client);
                }
                Err(_) => {
                    self.failed = true;
                    self.backlog.clear();
                }
            }
        }
    }
    pub(super) fn refresh(&mut self) {
        match &mut self.client {
            Some(client) => client.refresh(),
            None => self.open(),
        }
    }
    /// The shares with the request pending, once the stream is there.
    pub(super) fn submit(
        &mut self,
        request: ShareRequest,
    ) -> std::io::Result<Option<Arc<SharesSnapshot>>> {
        request.validate()?;
        if let Some(client) = &mut self.client {
            return client.submit(request).map(Some);
        }
        if self.failed || self.backlog.len() >= 16 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Sharing is not available",
            ));
        }
        self.backlog.push(request);
        self.open();
        Ok(None)
    }
    pub(super) async fn finish(mut self) -> std::io::Result<()> {
        if let Some(context) = self.setup_context.take() {
            context.close();
        }
        if let Some(setup) = self.setup.take() {
            self.client = setup.await.ok().map(SharesClient::new);
        }
        match self.client.take() {
            Some(client) => client.finish().await,
            None => Ok(()),
        }
    }
}
