use std::sync::Arc;

use crate::tab_stash::{SessionBucket, SessionPair, SleepingBudget, Text};

#[path = "tab_session_script.rs"]
mod script;

#[path = "tab_session_host.rs"]
pub mod native_host;

pub(crate) enum SessionOperation {
    Length,
    Key { index: usize },
    Get { key: Text },
    Set { key: Text, value: Text },
    Remove { key: Text },
    Clear,
    MarkSensitive { key: Text },
}

pub(crate) enum SessionReply {
    Length(usize),
    Key(Option<Text>),
    Value(Option<Text>),
    Complete,
}

pub(crate) enum SessionError {
    QuotaExceeded,
}

pub(crate) struct SessionStore {
    origin: url::Origin,
    backing: Option<Arc<SessionBucket>>,
    entries: Vec<SessionPair>,
    sensitive: Vec<Text>,
    no_stash: bool,
}

impl SessionStore {
    fn new(origin: url::Origin, backing: Option<Arc<SessionBucket>>) -> Self {
        Self {
            origin,
            backing,
            entries: Vec::new(),
            sensitive: Vec::new(),
            no_stash: false,
        }
    }

    fn pairs(&self) -> &[SessionPair] {
        self.backing
            .as_ref()
            .map_or(&self.entries, |bucket| bucket.pairs())
    }

    fn make_mut(&mut self) {
        if let Some(bucket) = self.backing.take() {
            self.entries = match Arc::try_unwrap(bucket) {
                Ok(bucket) => bucket.into_pairs(),
                Err(bucket) => bucket
                    .pairs()
                    .iter()
                    .map(|pair| SessionPair {
                        key: Text::capture(pair.key.units().iter().copied(), 2048)
                            .expect("bounded native key"),
                        value: Text::capture(pair.value.units().iter().copied(), 8192)
                            .expect("bounded native value"),
                    })
                    .collect(),
            };
        }
    }

    pub(crate) fn operate(
        &mut self,
        operation: SessionOperation,
    ) -> Result<SessionReply, SessionError> {
        let (key, value) = match &operation {
            SessionOperation::Get { key }
            | SessionOperation::Remove { key }
            | SessionOperation::MarkSensitive { key } => (Some(key), None),
            SessionOperation::Set { key, value } => (Some(key), Some(value)),
            _ => (None, None),
        };
        if key.is_some_and(|key| key.allocated_bytes() > 2048)
            || value.is_some_and(|value| value.allocated_bytes() > 8192)
        {
            return Err(SessionError::QuotaExceeded);
        }
        match operation {
            SessionOperation::Length => Ok(SessionReply::Length(self.pairs().len())),
            SessionOperation::Key { index } => {
                Ok(SessionReply::Key(self.pairs().get(index).and_then(
                    |pair| Text::capture(pair.key.units().iter().copied(), 2048),
                )))
            }
            SessionOperation::Get { key } => Ok(SessionReply::Value(
                self.pairs()
                    .iter()
                    .find(|pair| pair.key.units() == key.units())
                    .and_then(|pair| Text::capture(pair.value.units().iter().copied(), 8192)),
            )),
            SessionOperation::Set { key, value } => {
                self.make_mut();
                let bytes = self
                    .entries
                    .iter()
                    .filter(|pair| pair.key.units() != key.units())
                    .map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes())
                    .sum::<usize>();
                let replacing = self
                    .entries
                    .iter()
                    .position(|pair| pair.key.units() == key.units());
                if bytes + key.allocated_bytes() + value.allocated_bytes() > 24 * 1024
                    || replacing.is_none() && self.entries.len() >= 256
                {
                    return Err(SessionError::QuotaExceeded);
                }
                let pair = SessionPair { key, value };
                if let Some(index) = replacing {
                    self.entries[index] = pair;
                } else {
                    self.entries.push(pair);
                }
                Ok(SessionReply::Complete)
            }
            SessionOperation::Remove { key } => {
                self.make_mut();
                self.entries.retain(|pair| pair.key.units() != key.units());
                Ok(SessionReply::Complete)
            }
            SessionOperation::Clear => {
                self.backing.take();
                self.entries.clear();
                Ok(SessionReply::Complete)
            }
            SessionOperation::MarkSensitive { key } => {
                if !self.sensitive.iter().any(|old| old.units() == key.units()) {
                    if self.sensitive.len() >= 256 {
                        self.no_stash = true;
                        return Ok(SessionReply::Complete);
                    }
                    self.sensitive.push(key);
                }
                Ok(SessionReply::Complete)
            }
        }
    }

    fn capture(&mut self, budget: &Arc<SleepingBudget>) -> Option<Arc<SessionBucket>> {
        if self.no_stash {
            self.backing.take();
            self.entries.clear();
            return None;
        }
        if self.sensitive.is_empty() && self.backing.is_some() {
            return self.backing.take();
        }
        self.make_mut();
        self.entries.retain(|pair| {
            !self
                .sensitive
                .iter()
                .any(|key| key.units() == pair.key.units())
        });
        SessionBucket::seal(
            budget,
            self.origin.clone(),
            std::mem::take(&mut self.entries),
        )
        .ok()
    }
}

pub fn install(document: &mut blitz_script::ScriptDocument, backing: Option<Arc<SessionBucket>>) {
    script::install(document, backing);
}

pub fn capture(
    document: &mut blitz_script::ScriptDocument,
    budget: &Arc<SleepingBudget>,
) -> Option<Arc<SessionBucket>> {
    script::capture(document, budget)
}
