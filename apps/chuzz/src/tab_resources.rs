use std::sync::Arc;

use blitz_dom::release::{ReleaseLease, Teardown};
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

pub struct PageNet {
    pub net: Arc<crate::document_loader::NetProvider>,
    pub lifetime: Arc<Teardown>,
}

struct Callback {
    handler: Box<dyn NetHandler>,
    _lease: ReleaseLease,
}

impl NetHandler for Callback {
    fn bytes(self: Box<Self>, url: String, bytes: Bytes) {
        self.handler.bytes(url, bytes);
    }
}

impl NetProvider for PageNet {
    fn fetch(&self, document: usize, request: Request, handler: Box<dyn NetHandler>) {
        self.net.fetch(
            document,
            request,
            Box::new(Callback {
                handler,
                _lease: self.lifetime.lease(),
            }),
        );
    }
    fn is_noop(&self) -> bool {
        self.net.is_noop()
    }
}
