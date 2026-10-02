use super::*;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::mem::size_of;

pub(super) type Metadata = (
    Option<String>,
    Vec<String>,
    BTreeMap<String, SnippetInputSpec>,
    Option<SnippetToolDeclarations>,
    String,
);
const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = MAX_BYTES / 4;
type Entry = (String, String, bool, Result<Metadata, ToolError>, usize);
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Default for Cache {
    fn default() -> Self {
        let entries = VecDeque::with_capacity(MAX_ENTRIES);
        let bytes = entries.capacity() * size_of::<Entry>() + size_of::<Self>();
        Self { entries, bytes }
    }
}

// Conservatively charge an entire B-tree leaf per entry, including spare slots.
// This avoids depending on std/serde's private allocation layouts.
fn tree_entry_bytes<K, V>() -> usize {
    12 * (size_of::<K>() + size_of::<V>() + 4 * size_of::<usize>())
}
fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => {
            values.capacity() * size_of::<Value>() + values.iter().map(value_bytes).sum::<usize>()
        }
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                tree_entry_bytes::<String, Value>() + key.capacity() + value_bytes(value)
            })
            .sum(),
        _ => 0,
    }
}
fn metadata_bytes(metadata: &Result<Metadata, ToolError>) -> usize {
    match metadata {
        Ok((description, tags, inputs, tools, digest)) => {
            description.as_ref().map_or(0, String::capacity)
                + digest.capacity()
                + tags.capacity() * size_of::<String>()
                + tags.iter().map(String::capacity).sum::<usize>()
                + inputs
                    .iter()
                    .map(|(key, spec)| {
                        tree_entry_bytes::<String, SnippetInputSpec>()
                            + key.capacity()
                            + spec.description.as_ref().map_or(0, String::capacity)
                            + spec.default.as_ref().map_or(0, value_bytes)
                    })
                    .sum::<usize>()
                + tools.as_ref().map_or(0, |tools| {
                    tools
                        .as_slice()
                        .iter()
                        .map(|tool| size_of::<String>() + tool.capacity())
                        .sum()
                })
        }
        // Cached failures originate only in local parsing/validation. Clone stores
        // exact-length strings; serialized additive fields cover any future variants.
        Err(error) => {
            error.user_message().len()
                + serde_json::to_vec(&error.extra_fields()).map_or(0, |bytes| bytes.len() * 12)
        }
    }
}
impl Cache {
    fn insert(
        &mut self,
        name: String,
        digest: String,
        builtin: bool,
        metadata: Result<Metadata, ToolError>,
    ) {
        let charge = name.capacity() + digest.capacity() + metadata_bytes(&metadata);
        // A changed revision of the same snippet replaces its previous charge.
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.0 == name && entry.2 == builtin)
        {
            let old = self.entries.remove(index).expect("existing cache position");
            self.bytes -= old.4;
        }
        // Large metadata must not churn every other cached snippet.
        if charge > MAX_ENTRY_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + charge > MAX_BYTES {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.4;
            } else {
                break;
            }
        }
        self.bytes += charge;
        self.entries
            .push_back((name, digest, builtin, metadata, charge));
    }
}
pub(super) fn digest(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}
pub(super) fn metadata(
    name: &str,
    body: &str,
    source: SnippetSource,
) -> Result<Metadata, ToolError> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let digest = digest(body);
    let builtin = source == SnippetSource::Builtin;
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Cache::default()))
        .lock()
        .map_err(|_| ToolError::internal_message("snippet metadata cache poisoned"))?;
    if let Some(index) = cache
        .entries
        .iter()
        .position(|entry| entry.0 == name && entry.1 == digest && entry.2 == builtin)
    {
        let entry = cache.entries.remove(index).expect("cache hit position");
        let metadata = entry.3.clone();
        cache.entries.push_back(entry);
        return metadata;
    }
    let metadata = (|| {
        let frontmatter = frontmatter(body)?;
        if builtin && frontmatter.is_none() {
            return Err(ToolError::InvalidParam {
                message: "built-in snippet requires frontmatter".into(),
                param: "body".into(),
            });
        }
        validate_snippet_body(name, body)?;
        let (description, tags, inputs, tools) = snippet_metadata_fields(frontmatter);
        Ok((description, tags, inputs, tools, digest.clone()))
    })();
    cache.insert(name.to_string(), digest, builtin, metadata.clone());
    metadata
}
#[cfg(test)]
mod tests;
