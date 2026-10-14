//! Add the host's network wake to the shared in-process control transport.

use std::sync::Arc;
use std::task::Context;
use blitz_control_protocol::latest::Latest;
use blitz_control_protocol::DebugEvent;
use blitz_traits::net::Url;
use super::page_control;
#[path = "serve_supervisor.rs"]
mod transport;

pub(super) fn start_worker(sender: page_control::Sender) -> Result<(), String> { transport::start_worker(sender) }
pub(super) fn events() -> Arc<Latest<DebugEvent>> { transport::events() }
pub(super) fn ready(url: &Url) { transport::ready(url); }
pub(super) fn task_context() -> Context<'static> { transport::task_context() }

pub(super) struct NetworkWake;
impl blitz_traits::net::NetWaker for NetworkWake {
    fn wake(&self, _document: usize) { transport::network_wake().wake_by_ref(); }
}
pub(super) fn network_wake() -> Arc<NetworkWake> { Arc::new(NetworkWake) }

#[derive(Clone, Copy)]
pub(super) enum Operation { Load, Poll, Resolve, Action, Storage, Request }
pub(super) struct OperationGuard { _guard: transport::OperationGuard }
pub(super) fn operation(operation: Operation) -> OperationGuard {
    let operation = match operation {
        Operation::Load => transport::Operation::Load,
        Operation::Poll => transport::Operation::Poll,
        Operation::Resolve => transport::Operation::Resolve,
        Operation::Action => transport::Operation::Action,
        Operation::Storage => transport::Operation::Storage,
        Operation::Request => transport::Operation::Request,
    };
    OperationGuard { _guard: transport::operation(operation) }
}
