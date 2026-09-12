//! Syntactic normalization: the passes that need no replay.
//!
//! Canonical renaming collapses traces that differ only in which arbitrary
//! identifiers a search happened to pick. Commutativity reordering collapses
//! traces that differ only in the order of steps the target says do not
//! interact. Both are pure functions over the envelope.
//!
//! The replay-driven passes live in `verify-runner`; see the crate docs.

use serde_json::Value;

use crate::envelope::Scenario;

/// Whether two adjacent steps may be reordered without changing behavior.
///
/// Supplied by the caller because only the target knows. The default used
/// everywhere in this crate is "nothing commutes", which is always sound and
/// merely weaker at deduplication — wrongly declaring commutativity silently
/// merges genuinely distinct counterexamples.
pub trait Commutes {
    fn commutes(&self, a: &Value, b: &Value) -> bool;
}

/// A `Commutes` that never reorders. Sound for any target.
pub struct NeverCommutes;

impl Commutes for NeverCommutes {
    fn commutes(&self, _a: &Value, _b: &Value) -> bool {
        false
    }
}

/// Apply the syntactic passes, returning a new scenario.
///
/// Calling this opts into treating every identifier-shaped string value as an
/// arbitrary id. Use `normalize_commuting` for targets with literal payloads.
///
/// The input is not mutated: a caller comparing pre- and post-normalization
/// verdicts needs both.
pub fn normalize_syntactic(scenario: &Scenario, commutes: &dyn Commutes) -> Scenario {
    let mut out = scenario.clone();
    let mut renamer = Renamer::default();
    if let Some(initial) = out.initial.as_mut() {
        for value in initial.values_mut() {
            renamer.rewrite(value);
        }
    }
    for step in &mut out.steps {
        renamer.rewrite(step);
    }
    reorder_commuting(&mut out.steps, commutes);
    out
}

/// Reorder only declared commuting steps, preserving all opaque payload strings.
pub fn normalize_commuting(scenario: &Scenario, commutes: &dyn Commutes) -> Scenario {
    let mut out = scenario.clone();
    reorder_commuting(&mut out.steps, commutes);
    out
}

/// Renumbers identifier-shaped strings in order of first appearance, so
/// `{upstream_7, upstream_2}` and `{upstream_1, upstream_0}` collapse.
///
/// Only strings matching `prefix_<digits>` are touched. Renaming anything else
/// risks mangling a payload that happens to look like an id.
#[derive(Default)]
struct Renamer {
    assigned: std::collections::HashMap<String, String>,
    next: std::collections::HashMap<String, u32>,
}

impl Renamer {
    fn rewrite(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if let Some(canonical) = self.canonical(text) {
                    *text = canonical;
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.rewrite(item);
                }
            }
            Value::Object(map) => {
                for nested in map.values_mut() {
                    self.rewrite(nested);
                }
            }
            _ => {}
        }
    }

    fn canonical(&mut self, text: &str) -> Option<String> {
        let (prefix, suffix) = text.rsplit_once('_')?;
        if prefix.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if let Some(existing) = self.assigned.get(text) {
            return Some(existing.clone());
        }
        let counter = self.next.entry(prefix.to_owned()).or_default();
        let canonical = format!("{prefix}_{counter}");
        *counter += 1;
        self.assigned.insert(text.to_owned(), canonical.clone());
        Some(canonical)
    }
}

/// Bubble adjacent commuting steps into a canonical order.
///
/// Only *adjacent* pairs are swapped, and only where the target declares them
/// independent — a general sort would reorder steps across a non-commuting
/// step and change the trace's meaning.
fn reorder_commuting(steps: &mut [Value], commutes: &dyn Commutes) {
    if steps.len() < 2 {
        return;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for index in 1..steps.len() {
            let (left, right) = (&steps[index - 1], &steps[index]);
            if commutes.commutes(left, right) && canonical_order(right, left) {
                steps.swap(index - 1, index);
                changed = true;
            }
        }
    }
}

/// Total order over steps, used only to pick one representative of a set of
/// interchangeable orderings. Compares canonical text, which is stable across
/// runs and machines.
fn canonical_order(candidate: &Value, current: &Value) -> bool {
    crate::fingerprint::canonical_bytes(candidate) < crate::fingerprint::canonical_bytes(current)
}

/// Convenience for callers with no commutativity knowledge.
pub fn normalize_identifiers_only(scenario: &Scenario) -> Scenario {
    normalize_syntactic(scenario, &NeverCommutes)
}
