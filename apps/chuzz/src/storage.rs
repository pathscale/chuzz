use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::task::Waker;

const MAX_REALMS: usize = 4096;

#[derive(Default)]
struct ReadyState {
    wakers: HashMap<u64, Option<Waker>>,
    ready: HashSet<u64>,
}

static READY: LazyLock<Mutex<ReadyState>> = LazyLock::new(Mutex::default);

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(super) fn bind_platform(token: u64, waker: Option<&Waker>) -> Result<(), String> {
    let mut state = lock(&READY);
    if !state.wakers.contains_key(&token) && state.wakers.len() >= MAX_REALMS {
        return Err("platform realm limit reached".to_owned());
    }
    state.wakers.entry(token).or_insert(None);
    if let Some(waker) = waker {
        state.wakers.insert(token, Some(waker.clone()));
        if state.ready.contains(&token) {
            let wake = state.wakers.get(&token).cloned().flatten();
            drop(state);
            if let Some(wake) = wake {
                wake.wake();
            }
        }
    }
    Ok(())
}

pub(super) fn mark_platform_ready(token: u64) {
    let wake = {
        let mut state = lock(&READY);
        if !state.wakers.contains_key(&token) {
            return;
        }
        state.ready.insert(token);
        state.wakers.get(&token).cloned().flatten()
    };
    if let Some(wake) = wake {
        wake.wake();
    }
}

pub(super) fn take_platform_ready(token: u64) -> Result<bool, String> {
    Ok(lock(&READY).ready.remove(&token))
}

pub(super) fn unbind_platform(token: u64) {
    let mut state = lock(&READY);
    state.wakers.remove(&token);
    state.ready.remove(&token);
}

#[derive(Default)]
pub(super) struct PortState {
    pub ports: HashMap<u64, Port>,
}

pub(super) struct Port {
    pub owner: u64,
    pub peer: u64,
    pub started: bool,
    pub incoming: VecDeque<String>,
}

impl PortState {
    pub fn close_realm(&mut self, owner: u64) {
        let ids = self
            .ports
            .iter()
            .filter_map(|(id, port)| (port.owner == owner).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(port) = self.ports.remove(&id)
                && let Some(peer) = self.ports.get_mut(&port.peer)
            {
                peer.peer = 0;
            }
        }
    }
}
