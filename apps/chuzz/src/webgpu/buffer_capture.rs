use std::time::Instant;

use blitz_control_protocol::{DebugError, WebGpuBufferSnapshot};

use super::*;

const CAPTURE_LIMIT: usize = 16;
const RAW_LIMIT: u64 = ((blitz_control_protocol::MAX_DEBUG_FRAME_BYTES - 64 * 1024) / 4 * 3) as u64;

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) struct Key {
    pub target: String,
    pub offset: u64,
    pub length: Option<u64>,
}

pub(super) struct Slot {
    result: Mutex<Option<Result<WebGpuBufferSnapshot, DebugError>>>,
    expires: Instant,
    wake: crate::net_bridge::PageWake,
}

impl Slot {
    pub(super) fn finish(&self, result: Result<WebGpuBufferSnapshot, DebugError>) {
        let mut saved = lock(&self.result);
        if saved.is_none() {
            *saved = Some(result);
        }
        self.wake.wake();
    }

    fn take(&self) -> Option<Result<WebGpuBufferSnapshot, DebugError>> {
        lock(&self.result).take()
    }
}

fn error(code: &str, message: impl Into<String>) -> DebugError {
    DebugError {
        code: code.to_owned(),
        message: message.into(),
    }
}

fn label_pattern(target: &str) -> Result<&str, DebugError> {
    target
        .strip_prefix("buffer:")
        .filter(|label| !label.is_empty())
        .ok_or_else(|| {
            error(
                "invalidArgument",
                format!("unknown WebGPU buffer target: {target}"),
            )
        })
}

// Keep the same character-aware '*' semantics as ps-qa's GpuHasResource.
fn label_matches(pattern: &str, text: &str) -> bool {
    let Some((prefix, rest)) = pattern.split_once('*') else {
        return pattern == text;
    };
    if !text.starts_with(prefix) {
        return false;
    }
    let text = &text[prefix.len()..];
    if rest.is_empty() {
        return true;
    }
    let mut skip = 0;
    loop {
        if label_matches(rest, &text[skip..]) {
            return true;
        }
        let Some(next) = text[skip..].chars().next() else {
            return false;
        };
        skip += next.len_utf8();
    }
}

impl Bridge {
    /// The document only admits work or consumes a finished slot. Encoding,
    /// matching and GPU work stay on the existing ordered readback worker.
    pub(super) fn sample_buffer(
        &self,
        target: &str,
        offset: u64,
        length: Option<u64>,
    ) -> Result<WebGpuBufferSnapshot, DebugError> {
        if !self.inspection_active() {
            return Err(error("pending", "WebGPU inspection is inactive"));
        }
        label_pattern(target)?;
        if !offset.is_multiple_of(4) || length.is_some_and(|length| !length.is_multiple_of(4)) {
            return Err(error(
                "invalidArgument",
                format!("buffer {target:?} offset and length must be multiples of 4 bytes"),
            ));
        }
        let key = Key {
            target: target.to_owned(),
            offset,
            length,
        };
        let slot = {
            let mut state = self.state();
            state
                .buffer_captures
                .retain(|_, slot| slot.expires > Instant::now());
            if let Some(slot) = state.buffer_captures.get(&key) {
                if let Some(result) = slot.take() {
                    state.buffer_captures.remove(&key);
                    return result;
                }
                return Err(error(
                    "pending",
                    format!("buffer {target:?} sample is pending"),
                ));
            }
            if state.buffer_captures.len() >= CAPTURE_LIMIT {
                return Err(error(
                    "pending",
                    "WebGPU buffer capture request limit reached",
                ));
            }
            let slot = Arc::new(Slot {
                result: Mutex::new(None),
                expires: Instant::now() + DEADLINE,
                wake: state.wake.clone(),
            });
            state.buffer_captures.insert(key.clone(), Arc::clone(&slot));
            slot
        };
        self.enqueue(Request::BufferCapture {
            id: readback::next_capture_id(),
            key,
            slot,
        });
        Err(error(
            "pending",
            format!("buffer {target:?} sample is pending"),
        ))
    }

    pub(super) fn read_buffer_capture(
        &self,
        key: &Key,
    ) -> Result<WebGpuBufferSnapshot, DebugError> {
        if !self.inspection_active() {
            return Err(error(
                "pending",
                "WebGPU inspection expired before the buffer sample",
            ));
        }
        let pattern = label_pattern(&key.target)?;
        let devices = self.state().devices.values().cloned().collect::<Vec<_>>();
        let mut matches = Vec::new();
        for device in devices {
            let found = {
                let resources = device.resources();
                resources
                    .buffers
                    .keys()
                    .filter_map(|handle| {
                        let label = device.inspection_label(*handle);
                        (!label.is_empty() && label_matches(pattern, &label))
                            .then_some((*handle, label))
                    })
                    .collect::<Vec<_>>()
            };
            for (handle, label) in found {
                matches.push((Arc::clone(&device), handle, label));
            }
        }
        matches.sort_by(|a, b| (&a.2, a.0.id, a.1).cmp(&(&b.2, b.0.id, b.1)));
        if matches.is_empty() {
            return Err(error(
                "invalidArgument",
                format!("no WebGPU buffer matches label {pattern:?}"),
            ));
        }
        if matches.len() != 1 {
            let names = matches
                .iter()
                .map(|(device, handle, label)| {
                    format!("{label:?} (device {}, buffer {handle})", device.id)
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(error(
                "invalidArgument",
                format!("more than one WebGPU buffer matches label {pattern:?}: {names}"),
            ));
        }
        let (device, handle, label) = matches.pop().unwrap();
        if device.lost.load(Ordering::Acquire) {
            return Err(error(
                "internal",
                format!("GPUDevice for buffer {label:?} is lost or destroyed"),
            ));
        }
        let (size, usage, length, snapshot) = {
            let resources = device.resources();
            let entry = resources.buffers.get(&handle).ok_or_else(|| {
                error("invalidArgument", format!("buffer {label:?} was destroyed"))
            })?;
            let length = key
                .length
                .unwrap_or_else(|| entry.size.saturating_sub(key.offset));
            let end = key
                .offset
                .checked_add(length)
                .filter(|end| *end <= entry.size);
            if key.offset > entry.size || end.is_none() {
                return Err(error(
                    "invalidArgument",
                    format!(
                        "buffer {label:?} range offset={} length={length} exceeds size={}",
                        key.offset, entry.size
                    ),
                ));
            }
            if !length.is_multiple_of(4) {
                return Err(error(
                    "invalidArgument",
                    format!("buffer {label:?} length to the end must be a multiple of 4 bytes"),
                ));
            }
            if length > RAW_LIMIT {
                return Err(error(
                    "captureTooLarge",
                    format!("buffer {label:?} range cannot fit in one diagnostic frame"),
                ));
            }
            let history = entry.inspection.as_ref().ok_or_else(|| error("unsupported",
                format!("buffer {label:?} was created before GPU inspection; recreate it while inspection is active")))?;
            let newest = history.newest.load(Ordering::Acquire);
            let snapshot = lock(&history.completed).clone();
            if history.pending.load(Ordering::Acquire) == 0
                && snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.submit < newest)
                && device.inspection_active()
            {
                device.checkpoint_buffer(
                    entry,
                    Arc::clone(history),
                    newest,
                    history.written_by_write_buffer.load(Ordering::Relaxed),
                );
            }
            let snapshot = snapshot.or_else(|| lock(&history.completed).clone());
            let Some(snapshot) = snapshot else {
                if let Some(message) = lock(&history.error).clone() {
                    return Err(error(
                        "internal",
                        format!("buffer {label:?} readback failed: {message}"),
                    ));
                }
                return Err(error(
                    "pending",
                    format!("buffer {label:?} has no completed write sample yet (submit {newest})"),
                ));
            };
            (entry.size, entry.usage.bits(), length, snapshot)
        };
        let data_base64 = if length == 0 {
            String::new()
        } else {
            let staging = snapshot.staging.as_ref().ok_or_else(|| {
                error(
                    "internal",
                    format!("buffer {label:?} completed sample has no bytes"),
                )
            })?;
            let end = key.offset + length;
            if end > snapshot.bytes {
                return Err(error(
                    "internal",
                    format!("buffer {label:?} completed sample is too short"),
                ));
            }
            // Only map the aligned whole range; a requested four-byte offset
            // need not meet wgpu's eight-byte mapped-view offset alignment.
            let mapped = staging.get_mapped_range(0..snapshot.bytes);
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &mapped[key.offset as usize..end as usize],
            )
        };
        let snapshot = WebGpuBufferSnapshot {
            label,
            size,
            usage,
            offset: key.offset,
            submit: snapshot.submit,
            written_by_write_buffer: snapshot.written_by_write_buffer,
            data_base64,
        };
        // Account for escaped labels as well as bytes before filling the slot.
        let label_json = serde_json::to_string(&snapshot.label).map_err(|why| {
            error(
                "internal",
                format!("could not encode WebGPU buffer label: {why}"),
            )
        })?;
        if label_json.len() + snapshot.data_base64.len() + 256
            > blitz_control_protocol::MAX_DEBUG_FRAME_BYTES
        {
            return Err(error(
                "captureTooLarge",
                "WebGPU buffer snapshot exceeds the diagnostic frame limit",
            ));
        }
        Ok(snapshot)
    }
}
