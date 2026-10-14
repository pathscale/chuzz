//! Compare-exchange updates for atomics. `fetch_update` is deprecated on current stable and its
//! replacement is newer than the toolchains this crate still builds with.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub(crate) fn update_u64(
    cell: &AtomicU64,
    set: Ordering,
    fetch: Ordering,
    mut f: impl FnMut(u64) -> Option<u64>,
) -> Result<u64, u64> {
    let mut current = cell.load(fetch);
    loop {
        let next = f(current).ok_or(current)?;
        match cell.compare_exchange_weak(current, next, set, fetch) {
            Ok(previous) => return Ok(previous),
            Err(observed) => current = observed,
        }
    }
}

pub(crate) fn update_usize(
    cell: &AtomicUsize,
    set: Ordering,
    fetch: Ordering,
    mut f: impl FnMut(usize) -> Option<usize>,
) -> Result<usize, usize> {
    let mut current = cell.load(fetch);
    loop {
        let next = f(current).ok_or(current)?;
        match cell.compare_exchange_weak(current, next, set, fetch) {
            Ok(previous) => return Ok(previous),
            Err(observed) => current = observed,
        }
    }
}
