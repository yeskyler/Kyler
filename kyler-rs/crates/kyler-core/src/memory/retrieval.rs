use super::store::{MemoryEntry, MemoryStore};

use anyhow::Result;

impl MemoryStore {
    pub fn search(&self, scope: &str, query: &str) -> Result<Vec<MemoryEntry>> {
        self.transaction(scope, false, |entries| {
            let terms = query
                .to_lowercase()
                .split_whitespace()
                .map(str::to_owned)
                .take(32)
                .collect::<Vec<_>>();
            let mut items = entries
                .values()
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
                    .then_some((score, entry.clone()))
                })
                .collect::<Vec<_>>();
            items.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated_at.cmp(&a.1.updated_at)));
            Ok(items.into_iter().take(16).map(|(_, entry)| entry).collect())
        })
    }
}
