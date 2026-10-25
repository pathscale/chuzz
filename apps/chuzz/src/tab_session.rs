use std::sync::{Arc, Mutex, OnceLock};

use crate::tab_stash::{SessionBucket, SessionPair, SleepingBudget, Text};

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

/// Persistent localStorage for one origin. Keys and values stay as UTF-16
/// code units all the way to disk, so even lone surrogates and `__proto__` are
/// data rather than JavaScript object properties.
pub(crate) struct LocalStorageStore {
    origin: url::Origin,
    entries: Vec<SessionPair>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedLocalStorage {
    origins: Vec<PersistedOriginStorage>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedOriginStorage {
    origin: String,
    entries: Vec<(Vec<u16>, Vec<u16>)>,
}

static LOCAL_STORAGE_IO: OnceLock<Mutex<()>> = OnceLock::new();

impl LocalStorageStore {
    pub(crate) fn new(origin: url::Origin) -> Self {
        let mut store = Self {
            origin,
            entries: Vec::new(),
        };
        let _ = store.refresh();
        store
    }

    pub(crate) fn length(&mut self) -> Result<usize, LocalStorageError> {
        self.refresh()?;
        Ok(self.entries.len())
    }

    pub(crate) fn key(&mut self, index: usize) -> Result<Option<&[u16]>, LocalStorageError> {
        self.refresh()?;
        Ok(self.entries.get(index).map(|pair| pair.key.units()))
    }

    pub(crate) fn get(&mut self, key: &[u16]) -> Result<Option<&[u16]>, LocalStorageError> {
        self.refresh()?;
        Ok(self
            .entries
            .iter()
            .find(|pair| pair.key.units() == key)
            .map(|pair| pair.value.units()))
    }

    pub(crate) fn set(&mut self, key: Text, value: Text) -> Result<(), LocalStorageError> {
        self.refresh()?;
        validate_local_pair(&key, &value)?;
        let replacing = self
            .entries
            .iter()
            .position(|pair| pair.key.units() == key.units());
        let bytes = self
            .entries
            .iter()
            .filter(|pair| pair.key.units() != key.units())
            .map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes())
            .sum::<usize>();
        if bytes
            .saturating_add(key.allocated_bytes())
            .saturating_add(value.allocated_bytes())
            > 24 * 1024
            || replacing.is_none() && self.entries.len() >= 256
        {
            return Err(LocalStorageError::QuotaExceeded);
        }
        let pair = SessionPair { key, value };
        if let Some(index) = replacing {
            self.entries[index] = pair;
        } else {
            self.entries.push(pair);
        }
        self.persist()
    }

    pub(crate) fn remove(&mut self, key: &[u16]) -> Result<(), LocalStorageError> {
        self.refresh()?;
        self.entries.retain(|pair| pair.key.units() != key);
        self.persist()
    }

    pub(crate) fn clear(&mut self) -> Result<(), LocalStorageError> {
        self.refresh()?;
        self.entries.clear();
        self.persist()
    }

    pub(crate) fn seed(&mut self, entries: &[(String, String)]) -> Result<(), LocalStorageError> {
        if entries.len() > 256 {
            return Err(LocalStorageError::QuotaExceeded);
        }
        let mut seeded = Vec::with_capacity(entries.len());
        let mut bytes = 0usize;
        for (key, value) in entries {
            let key =
                Text::capture(key.encode_utf16(), 2048).ok_or(LocalStorageError::QuotaExceeded)?;
            let value = Text::capture(value.encode_utf16(), 8192)
                .ok_or(LocalStorageError::QuotaExceeded)?;
            bytes = bytes
                .saturating_add(key.allocated_bytes())
                .saturating_add(value.allocated_bytes());
            if bytes > 24 * 1024 {
                return Err(LocalStorageError::QuotaExceeded);
            }
            seeded.push(SessionPair { key, value });
        }
        self.entries = seeded;
        self.persist()
    }

    fn refresh(&mut self) -> Result<(), LocalStorageError> {
        if !matches!(self.origin, url::Origin::Tuple(..)) {
            return Ok(());
        }
        let _guard = local_storage_lock()?;
        self.entries = read_local_storage()?
            .origins
            .into_iter()
            .find(|record| record.origin == self.origin.ascii_serialization())
            .and_then(|record| {
                record
                    .entries
                    .into_iter()
                    .map(|(key, value)| {
                        Some(SessionPair {
                            key: Text::capture(key.into_iter(), 2048)?,
                            value: Text::capture(value.into_iter(), 8192)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|entries| valid_local_pairs(entries))
            .unwrap_or_default();
        Ok(())
    }

    fn persist(&self) -> Result<(), LocalStorageError> {
        if !matches!(self.origin, url::Origin::Tuple(..)) {
            return Ok(());
        }
        let _guard = local_storage_lock()?;
        let origin = self.origin.ascii_serialization();
        let mut persisted = read_local_storage()?;
        persisted.origins.retain(|record| record.origin != origin);
        if !self.entries.is_empty() {
            persisted.origins.push(PersistedOriginStorage {
                origin,
                entries: self
                    .entries
                    .iter()
                    .map(|pair| (pair.key.units().to_vec(), pair.value.units().to_vec()))
                    .collect(),
            });
        }
        write_local_storage(&persisted)
    }
}

fn validate_local_pair(key: &Text, value: &Text) -> Result<(), LocalStorageError> {
    if key.allocated_bytes() > 2048 || value.allocated_bytes() > 8192 {
        Err(LocalStorageError::QuotaExceeded)
    } else {
        Ok(())
    }
}

fn valid_local_pairs(entries: &[SessionPair]) -> bool {
    entries.len() <= 256
        && entries
            .iter()
            .all(|pair| pair.key.allocated_bytes() <= 2048 && pair.value.allocated_bytes() <= 8192)
        && entries
            .iter()
            .map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes())
            .sum::<usize>()
            <= 24 * 1024
}

#[derive(Clone, Copy)]
pub(crate) enum LocalStorageError {
    QuotaExceeded,
    Persistence,
}

fn local_storage_lock() -> Result<std::sync::MutexGuard<'static, ()>, LocalStorageError> {
    LOCAL_STORAGE_IO
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| LocalStorageError::Persistence)
}

fn local_storage_path() -> std::path::PathBuf {
    crate::cookie_store::profile_directory().join("local-storage.json")
}

fn read_local_storage() -> Result<PersistedLocalStorage, LocalStorageError> {
    let path = local_storage_path();
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| LocalStorageError::Persistence),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PersistedLocalStorage {
            origins: Vec::new(),
        }),
        Err(_) => Err(LocalStorageError::Persistence),
    }
}

fn write_local_storage(value: &PersistedLocalStorage) -> Result<(), LocalStorageError> {
    let path = local_storage_path();
    let parent = path.parent().ok_or(LocalStorageError::Persistence)?;
    std::fs::create_dir_all(parent).map_err(|_| LocalStorageError::Persistence)?;
    let encoded = serde_json::to_vec(value).map_err(|_| LocalStorageError::Persistence)?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, encoded).map_err(|_| LocalStorageError::Persistence)?;
    std::fs::rename(&temporary, &path).map_err(|_| LocalStorageError::Persistence)
}
