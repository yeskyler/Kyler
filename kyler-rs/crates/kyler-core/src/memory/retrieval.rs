use super::store::{MemoryEntry, MemoryStatus, MemoryStore};

use anyhow::Result;

const RECORD_BUDGET_BYTES: usize = 16 * 1024;

fn eligible(entry: &MemoryEntry, now: i64) -> bool {
    entry.status == MemoryStatus::Active
        && !entry.metadata.sensitive
        && entry.metadata.expires_at.is_none_or(|expiry| expiry > now)
}

fn fit_records<'a>(
    entries: impl Iterator<Item = &'a MemoryEntry>,
    max_records: usize,
) -> Vec<MemoryEntry> {
    let mut used = 0;
    entries
        .filter_map(|entry| {
            let size = serde_json::to_vec(entry).ok()?.len();
            if used + size > RECORD_BUDGET_BYTES {
                return None;
            }
            used += size;
            Some(entry.clone())
        })
        .take(max_records)
        .collect()
}

impl MemoryStore {
    /// Load active workspace history within the context byte budget.
    pub(crate) fn load_context(&self, scope: &str) -> Result<Vec<MemoryEntry>> {
        self.transaction(scope, false, |entries| {
            let now = chrono::Utc::now().timestamp_millis();
            let mut active: Vec<_> = entries
                .values()
                .filter(|entry| eligible(entry, now))
                .collect();
            active.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.key.cmp(&b.key)));
            Ok(fit_records(active.into_iter(), usize::MAX))
        })
    }

    pub fn search(&self, scope: &str, query: &str) -> Result<Vec<MemoryEntry>> {
        self.transaction(scope, false, |entries| {
            let terms = query
                .to_lowercase()
                .split_whitespace()
                .map(str::to_owned)
                .take(32)
                .collect::<Vec<_>>();
            let now = chrono::Utc::now().timestamp_millis();
            let mut items = entries
                .values()
                .filter(|entry| eligible(entry, now))
                .filter_map(|entry| {
                    let text = format!("{} {} {}", entry.key, entry.tags.join(" "), entry.content)
                        .to_lowercase();
                    let score = terms
                        .iter()
                        .filter(|term| text.contains(term.as_str()))
                        .count();
                    (query.is_empty()
                        || score > 0
                        || entry.tags.iter().any(|tag| tag == "preference"))
                    .then_some((score, entry))
                })
                .collect::<Vec<_>>();
            items.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated_at.cmp(&a.1.updated_at)));
            Ok(fit_records(items.into_iter().map(|(_, entry)| entry), 16))
        })
    }
}
