use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TabId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentGeneration(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Residency {
    Dormant,
    Loading,
    Live,
}

/// Public metadata contains no page values, request bodies or DOM ownership.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabRecord {
    pub id: u64,
    pub generation: u64,
    pub residency: Residency,
}

/// One policy owner. Records never contain an engine, DOM, callback or task.
pub struct TabRegistry {
    records: BTreeMap<TabId, TabRecord>,
    limit: usize,
    warn_at: usize,
}

impl Default for TabRegistry {
    fn default() -> Self {
        Self::new(20_000, 16_000)
    }
}

impl TabRegistry {
    pub fn new(limit: usize, warn_at: usize) -> Self {
        assert!(limit > 0 && warn_at > 0 && warn_at <= limit);
        Self { records: BTreeMap::new(), limit, warn_at }
    }

    pub fn admit(&mut self, id: TabId) -> Result<(), &'static str> {
        if self.records.contains_key(&id) {
            return Err("tab-id-already-present");
        }
        if self.records.len() >= self.limit {
            return Err("tab-record-budget");
        }
        self.records.insert(id, TabRecord {
            id: id.0,
            generation: 0,
            residency: Residency::Dormant,
        });
        Ok(())
    }

    pub fn loading(&mut self, id: TabId, generation: DocumentGeneration) {
        if let Some(record) = self.records.get_mut(&id) {
            record.generation = generation.0;
            record.residency = Residency::Loading;
        }
    }

    pub fn attached(&mut self, id: TabId, generation: DocumentGeneration) {
        if let Some(record) = self.records.get_mut(&id)
            && record.generation == generation.0
        {
            record.residency = Residency::Live;
        }
    }

    pub fn remove(&mut self, id: TabId) {
        self.records.remove(&id);
    }

    pub fn record(&self, id: TabId) -> Option<TabRecord> {
        self.records.get(&id).copied()
    }

    pub fn warning(&self) -> bool {
        self.records.len() >= self.warn_at
    }
}
