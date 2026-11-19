//! Bounded, field-weighted BM25 retrieval over one immutable authorized catalog.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Mutex,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize)]
pub struct ToolMetadata {
    pub name: String,
    pub server_id: String,
    pub remote_name: String,
    pub description: String,
    pub parameters: Value,
    /// Opaque identity including connection and schema generation; never a credential.
    pub version: String,
}
/// Search summaries retain no full schemas or connection handles.
pub struct IndexedTool {
    pub name: String,
    pub server_id: String,
    pub remote_name: String,
    pub description: String,
    pub version: String,
    pub required_parameters: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub name: String,
    pub server_id: String,
    pub remote_name: String,
    pub description: String,
    pub required_parameters: Vec<String>,
    pub score: f64,
    pub matched_fields: Vec<String>,
}
#[derive(Clone, PartialEq, Eq)]
struct SearchKey {
    queries: Vec<String>,
    servers: Vec<String>,
    limit: usize,
}
struct Posting {
    document: usize,
    frequency: f64,
    fields: u8,
}
pub struct ToolIndex {
    pub version: String,
    documents: Vec<IndexedTool>,
    postings: HashMap<String, Vec<Posting>>,
    cache: Mutex<VecDeque<(SearchKey, Vec<SearchHit>)>>,
}
impl ToolIndex {
    pub fn new(mut documents: Vec<ToolMetadata>) -> Result<Self> {
        ensure!(documents.len() <= 8192, "tool index exceeds 8192 tools");
        documents.sort_by(|a, b| a.name.cmp(&b.name));
        ensure!(
            documents.windows(2).all(|w| w[0].name != w[1].name),
            "duplicate indexed tool name"
        );
        let version = format!("{:x}", Sha256::digest(serde_json::to_vec(&documents)?));
        let fields: Vec<_> = documents
            .iter()
            .map(|doc| {
                let mut schema = Vec::new();
                schema_terms(&doc.parameters, &mut schema, 0);
                [
                    frequencies(&format!(
                        "{} {} {}",
                        doc.name, doc.remote_name, doc.server_id
                    )),
                    frequencies(&doc.description.chars().take(2048).collect::<String>()),
                    frequencies(&schema.join(" ")),
                ]
            })
            .collect();
        let mut averages = [0.0; 3];
        for doc in &fields {
            for (i, field) in doc.iter().enumerate() {
                averages[i] += field.values().sum::<usize>() as f64;
            }
        }
        for avg in &mut averages {
            *avg = (*avg / documents.len().max(1) as f64).max(1.0);
        }
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::new();
        for (document, doc) in fields.iter().enumerate() {
            let mut combined: HashMap<String, (f64, u8)> = HashMap::new();
            for (i, field) in doc.iter().enumerate() {
                let norm = 0.25 + 0.75 * field.values().sum::<usize>() as f64 / averages[i];
                for (term, count) in field {
                    let entry = combined.entry(term.clone()).or_default();
                    entry.0 += [5.0, 2.0, 1.0][i] * (*count).min(3) as f64 / norm;
                    entry.1 |= 1 << i;
                }
            }
            for (term, (frequency, fields)) in combined {
                postings.entry(term).or_default().push(Posting {
                    document,
                    frequency,
                    fields,
                });
            }
        }
        let documents = documents
            .into_iter()
            .map(|doc| IndexedTool {
                name: doc.name,
                server_id: doc.server_id,
                remote_name: doc.remote_name,
                description: doc.description.chars().take(240).collect(),
                version: doc.version,
                required_parameters: doc.parameters["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .take(32)
                    .map(str::to_owned)
                    .collect(),
            })
            .collect();
        Ok(Self {
            version,
            documents,
            postings,
            cache: Mutex::new(VecDeque::new()),
        })
    }
    pub fn documents(&self) -> &[IndexedTool] {
        &self.documents
    }
    /// Each query retrieves at most 30 candidates; reciprocal rank fusion balances intents.
    pub fn search(
        &self,
        queries: &[String],
        servers: &[String],
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        ensure!(
            (1..=4).contains(&queries.len())
                && queries
                    .iter()
                    .all(|q| !q.trim().is_empty() && q.chars().count() <= 4096),
            "provide 1..=4 nonempty queries of at most 4096 characters"
        );
        ensure!(
            (1..=30).contains(&limit) && servers.len() <= 32,
            "search limit must be in 1..=30 and server filter in 0..=32"
        );
        let mut servers = servers.to_vec();
        servers.sort();
        servers.dedup();
        let key = SearchKey {
            queries: queries.iter().map(|q| q.trim().to_lowercase()).collect(),
            servers,
            limit,
        };
        if let Some((_, hits)) = self
            .cache
            .lock()
            .unwrap()
            .iter()
            .find(|(cached, _)| cached == &key)
        {
            return Ok(hits.clone());
        }
        let mut fused: HashMap<usize, (f64, u8)> = HashMap::new();
        for query in &key.queries {
            let terms = query_terms(query);
            let mut scores: HashMap<usize, (f64, u8)> = HashMap::new();
            for (term, weight) in terms {
                let Some(postings) = self.postings.get(&term) else {
                    continue;
                };
                let n = self.documents.len() as f64;
                let df = postings.len() as f64;
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                for posting in postings {
                    let doc = &self.documents[posting.document];
                    if !key.servers.is_empty() && !key.servers.contains(&doc.server_id) {
                        continue;
                    }
                    let entry = scores.entry(posting.document).or_default();
                    entry.0 += weight * idf * posting.frequency * 2.2 / (posting.frequency + 1.2);
                    entry.1 |= posting.fields;
                }
            }
            for (index, doc) in self.documents.iter().enumerate() {
                if !key.servers.is_empty() && !key.servers.contains(&doc.server_id) {
                    continue;
                }
                if query == &doc.name.to_lowercase() || query == &doc.remote_name.to_lowercase() {
                    let entry = scores.entry(index).or_default();
                    entry.0 += 100.0;
                    entry.1 |= 1;
                }
            }
            let mut ranked: Vec<_> = scores.into_iter().filter(|(_, s)| s.0 > 0.0).collect();
            ranked.sort_by(|(a, sa), (b, sb)| {
                sb.0.total_cmp(&sa.0)
                    .then_with(|| self.documents[*a].name.cmp(&self.documents[*b].name))
            });
            for (rank, (index, (_, fields))) in ranked.into_iter().take(30).enumerate() {
                let entry = fused.entry(index).or_default();
                entry.0 += 1.0 / (60.0 + rank as f64);
                entry.1 |= fields;
            }
        }
        let mut ranked: Vec<_> = fused.into_iter().collect();
        ranked.sort_by(|(a, sa), (b, sb)| {
            sb.0.total_cmp(&sa.0)
                .then_with(|| self.documents[*a].name.cmp(&self.documents[*b].name))
        });
        let hits: Vec<_> = ranked
            .into_iter()
            .take(limit)
            .map(|(index, (score, fields))| {
                let doc = &self.documents[index];
                SearchHit {
                    name: doc.name.clone(),
                    server_id: doc.server_id.clone(),
                    remote_name: doc.remote_name.clone(),
                    description: doc.description.chars().take(240).collect(),
                    required_parameters: doc.required_parameters.clone(),
                    score,
                    matched_fields: ["name", "description", "parameters"]
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| fields & (1 << i) != 0)
                        .map(|(_, name)| name.to_owned())
                        .collect(),
                }
            })
            .collect();
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= 32 {
            cache.pop_front();
        }
        cache.push_back((key, hits.clone()));
        Ok(hits)
    }
}
fn schema_terms(value: &Value, terms: &mut Vec<String>, depth: usize) {
    if depth > 8 || terms.len() >= 256 {
        return;
    }
    if let Some(description) = value["description"].as_str() {
        terms.push(description.chars().take(240).collect());
    }
    if let Some(values) = value["enum"].as_array() {
        for value in values.iter().take(16) {
            if let Some(text) = value.as_str() {
                terms.push(text.chars().take(80).collect());
            }
        }
    }
    if let Some(properties) = value["properties"].as_object() {
        for (name, property) in properties {
            if terms.len() >= 256 {
                break;
            }
            terms.push(name.chars().take(128).collect());
            schema_terms(property, terms, depth + 1);
        }
    }
    for name in ["items", "additionalProperties"] {
        if let Some(child) = value.get(name) {
            schema_terms(child, terms, depth + 1);
        }
    }
    for name in ["oneOf", "anyOf", "allOf"] {
        if let Some(children) = value[name].as_array() {
            for child in children.iter().take(16) {
                schema_terms(child, terms, depth + 1);
            }
        }
    }
}
fn is_cjk(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut previous = None;
    let flush = |word: &mut String, tokens: &mut Vec<String>| {
        if !word.is_empty() {
            tokens.push(std::mem::take(word));
        }
    };
    let chars: Vec<_> = text.chars().take(8192).collect();
    for (i, &c) in chars.iter().enumerate() {
        if is_cjk(c) {
            flush(&mut word, &mut tokens);
            tokens.push(c.to_string());
            if let Some(&next) = chars.get(i + 1).filter(|next| is_cjk(**next)) {
                tokens.push(format!("{c}{next}"));
            }
        } else if c.is_alphanumeric() {
            if c.is_uppercase() && previous.is_some_and(char::is_lowercase) {
                flush(&mut word, &mut tokens);
            }
            word.extend(c.to_lowercase());
        } else {
            flush(&mut word, &mut tokens);
        }
        previous = Some(c);
    }
    flush(&mut word, &mut tokens);
    // Retain compact identifiers as well as their camel-case components.
    tokens.extend(
        text.split(|c: char| !c.is_alphanumeric() || is_cjk(c))
            .filter(|word| word.chars().any(char::is_uppercase))
            .take(256)
            .map(str::to_lowercase),
    );
    tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                t.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "to"
                    | "for"
                    | "of"
                    | "and"
                    | "please"
                    | "我"
                    | "的"
                    | "请"
                    | "帮"
            )
        })
        .take(1024)
        .collect()
}
fn frequencies(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for token in tokenize(text) {
        *counts.entry(token).or_default() += 1;
    }
    counts
}
fn query_terms(query: &str) -> BTreeMap<String, f64> {
    let mut terms = BTreeMap::new();
    for term in tokenize(query).into_iter().take(128) {
        let weight = if term.chars().count() == 1 && term.chars().next().is_some_and(is_cjk) {
            0.2
        } else {
            1.0
        };
        terms.insert(term, weight);
    }
    let groups: &[(&[&str], &[&str])] = &[
        (
            &["查找", "搜索", "search", "find", "lookup"],
            &["search", "find", "lookup"],
        ),
        (&["列出", "list"], &["list"]),
        (&["邮件", "email", "mail"], &["email", "mail", "message"]),
        (&["工单", "issue", "ticket"], &["issue", "ticket"]),
        (&["创建", "新建", "create"], &["create"]),
        (&["更新", "修改", "update"], &["update", "edit"]),
        (&["删除", "delete"], &["delete", "remove"]),
        (
            &["日历", "会议", "calendar", "meeting"],
            &["calendar", "meeting", "event"],
        ),
        (&["客户", "customer", "contact"], &["customer", "contact"]),
        (&["文件", "file", "document"], &["file", "document"]),
    ];
    for (triggers, expansions) in groups {
        if triggers.iter().any(|trigger| {
            if trigger.chars().any(is_cjk) {
                query.contains(trigger)
            } else {
                terms.contains_key(*trigger)
            }
        }) {
            for term in *expansions {
                terms.entry((*term).into()).or_insert(0.4);
            }
        }
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;
    fn doc(server: &str, name: &str, description: &str) -> ToolMetadata {
        ToolMetadata {
            name: format!("mcp__{server}__{name}"),
            server_id: server.into(),
            remote_name: name.into(),
            description: description.into(),
            parameters: json!({"type":"object","properties":{"customerEmail":{"type":"string","description":"Customer email address"}},"required":["customerEmail"]}),
            version: "one".into(),
        }
    }
    #[test]
    fn multilingual_queries_exact_names_filters_and_multiple_intents() {
        let index = ToolIndex::new(vec![
            doc("mail", "searchEmails", "Find email messages"),
            doc("tracker", "createIssue", "Create a ticket"),
            doc("calendar", "listEvents", "List calendar events"),
        ])
        .unwrap();
        assert_eq!(
            index.search(&["查找邮件".into()], &[], 6).unwrap()[0].remote_name,
            "searchEmails"
        );
        assert_eq!(
            index.search(&["createIssue".into()], &[], 6).unwrap()[0].remote_name,
            "createIssue"
        );
        let results = index
            .search(&["查找邮件".into(), "创建工单".into()], &[], 2)
            .unwrap();
        let names: HashSet<_> = results.iter().map(|hit| hit.remote_name.as_str()).collect();
        assert!(names.contains("searchEmails") && names.contains("createIssue"));
        assert!(
            index
                .search(&["查找邮件".into()], &["missing".into()], 6)
                .unwrap()
                .is_empty()
        );
        assert!(
            index
                .search(&["unrelatedzz".into()], &[], 6)
                .unwrap()
                .is_empty()
        );
        assert!(
            !index.search(&["customerEmail".into()], &[], 1).unwrap()[0]
                .required_parameters
                .is_empty()
        );
    }
    #[test]
    fn versions_and_caches_are_catalog_local() {
        let first = ToolIndex::new(vec![doc("mail", "searchEmails", "Find emails")]).unwrap();
        let mut changed = doc("mail", "searchEmails", "Find emails");
        changed.version = "two".into();
        let second = ToolIndex::new(vec![changed]).unwrap();
        assert_ne!(first.version, second.version);
        let hits = first.search(&["email".into()], &[], 2).unwrap();
        assert_eq!(
            hits[0].name,
            first.search(&["email".into()], &[], 2).unwrap()[0].name
        );
        assert!(ToolIndex::new(vec![doc("mail", "same", "a"), doc("mail", "same", "b")]).is_err());
    }
}
