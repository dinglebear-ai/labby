//! Session-owned VM cleanup; a tunnel never grants general destructive access.

use serde_json::{Map, Value};
use std::{collections::HashMap, sync::Mutex};

const OWNER_LABEL: &str = "labby-tailcat-owner";
const MAX_SANDBOXES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Owned,
    Removing,
}

/// Native-only capability. Never deserialize this from MCP metadata or headers.
pub(crate) struct CleanupSession {
    upstream: String,
    owner: String,
    sandboxes: Mutex<HashMap<String, State>>,
}

/// A single, reserved deletion of one sandbox, minted after successful creation.
pub(crate) struct CleanupPermit<'a> {
    session: &'a CleanupSession,
    name: String,
    dispatched: bool,
}

impl CleanupSession {
    pub(crate) fn new(upstream: &str) -> Self {
        Self {
            upstream: upstream.into(),
            owner: uuid::Uuid::new_v4().to_string(),
            sandboxes: Mutex::new(HashMap::new()),
        }
    }
    pub(crate) fn owner_label(&self) -> &str {
        &self.owner
    }
    pub(crate) fn upstream(&self) -> &str {
        &self.upstream
    }
    pub(crate) fn prepare_create(&self, args: &mut Map<String, Value>) -> Result<String, ()> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or(())?
            .to_owned();
        let suffix = name.strip_prefix("labby-tailcat-").ok_or(())?;
        if suffix.len() != 32
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(());
        }
        if let Some(lifecycle) = args.get("lifecycle") {
            let lifecycle = lifecycle.as_object().ok_or(())?;
            if lifecycle.contains_key("replaceTimeoutMs")
                || lifecycle
                    .get("replace")
                    .is_some_and(|value| value != &Value::Bool(false))
            {
                return Err(());
            }
        }
        let process = args
            .entry("process")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or(())?;
        let labels = process
            .entry("labels")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or(())?;
        let mut sandboxes = self.sandboxes.lock().map_err(|_| ())?;
        if sandboxes.len() >= MAX_SANDBOXES || sandboxes.contains_key(&name) {
            return Err(());
        }
        labels.insert(OWNER_LABEL.into(), Value::String(self.owner.clone()));
        sandboxes.insert(name.clone(), State::Pending);
        Ok(name)
    }
    pub(crate) fn finish_create(&self, name: &str, successful: bool) {
        if !successful {
            return;
        }
        if let Ok(mut sandboxes) = self.sandboxes.lock() {
            if sandboxes.get(name) == Some(&State::Pending) {
                sandboxes.insert(name.into(), State::Owned);
            }
        }
    }
    #[cfg(test)]
    fn owns(&self, name: &str) -> bool {
        self.sandboxes
            .lock()
            .is_ok_and(|s| s.get(name) == Some(&State::Owned))
    }
    pub(crate) fn matches_labels(&self, labels: &Value) -> bool {
        labels.get(OWNER_LABEL).and_then(Value::as_str) == Some(self.owner.as_str())
    }
    pub(crate) fn reserve_remove(
        &self,
        args: &Map<String, Value>,
    ) -> Result<CleanupPermit<'_>, ()> {
        if args.keys().any(|key| key != "name" && key != "force")
            || args.get("force").is_some_and(|v| !v.is_boolean())
        {
            return Err(());
        }
        let name = args.get("name").and_then(Value::as_str).ok_or(())?;
        let mut sandboxes = self.sandboxes.lock().map_err(|_| ())?;
        if sandboxes.get(name) != Some(&State::Owned) {
            return Err(());
        }
        sandboxes.insert(name.into(), State::Removing);
        Ok(CleanupPermit {
            session: self,
            name: name.into(),
            dispatched: false,
        })
    }
    fn removed(&self, name: &str) {
        if let Ok(mut sandboxes) = self.sandboxes.lock() {
            sandboxes.remove(name);
        }
    }
}
impl Drop for CleanupPermit<'_> {
    fn drop(&mut self) {
        if !self.dispatched {
            if let Ok(mut sandboxes) = self.session.sandboxes.lock() {
                if sandboxes.get(&self.name) == Some(&State::Removing) {
                    sandboxes.insert(self.name.clone(), State::Owned);
                }
            }
        }
    }
}
impl CleanupPermit<'_> {
    pub(crate) fn mark_dispatched(&mut self) {
        self.dispatched = true;
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn allows(
        &self,
        upstream: &str,
        native_name: &str,
        args: Option<&Map<String, Value>>,
    ) -> bool {
        upstream == self.session.upstream
            && native_name == "sandbox_remove"
            && args.and_then(|a| a.get("name")).and_then(Value::as_str) == Some(self.name.as_str())
            && self
                .session
                .sandboxes
                .lock()
                .is_ok_and(|s| s.get(&self.name) == Some(&State::Removing))
    }
    pub(crate) fn removed(self) {
        self.session.removed(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleanup_requires_successful_creation_and_same_session() {
        let owner = CleanupSession::new("msb");
        let other = CleanupSession::new("msb");
        let name = "labby-tailcat-0123456789abcdef0123456789abcdef";
        let mut args = json!({"name":name}).as_object().unwrap().clone();
        owner.prepare_create(&mut args).unwrap();
        assert!(!owner.owns(name));
        assert!(!other.owns(name));
        owner.finish_create(name, false);
        assert!(!owner.owns(name));
        assert!(owner.prepare_create(&mut args).is_err());
        let name = "labby-tailcat-1123456789abcdef0123456789abcdef";
        args.insert("name".into(), json!(name));
        owner.prepare_create(&mut args).unwrap();
        owner.finish_create(name, true);
        assert!(owner.owns(name));
        assert!(!other.owns(name));
        owner.removed(name);
        assert!(!owner.owns(name));
    }

    #[test]
    fn cleanup_permit_is_exact_exclusive_and_retired() {
        let owner = CleanupSession::new("msb");
        let name = "labby-tailcat-0123456789abcdef0123456789abcdef";
        let mut args =
            json!({"name":name,"process":{"labels":{"labby-tailcat-owner":"forged","keep":"yes"}}})
                .as_object()
                .unwrap()
                .clone();
        owner.prepare_create(&mut args).unwrap();
        assert!(owner.matches_labels(&args["process"]["labels"]));
        assert_eq!(args["process"]["labels"]["keep"], "yes");
        assert!(
            owner
                .reserve_remove(&json!({"name":name}).as_object().unwrap().clone())
                .is_err()
        );
        owner.finish_create(name, true);
        let remove = json!({"name":name,"force":true})
            .as_object()
            .unwrap()
            .clone();
        assert!(
            owner
                .reserve_remove(&json!({"name":name,"all":true}).as_object().unwrap().clone())
                .is_err()
        );
        let permit = owner.reserve_remove(&remove).unwrap();
        assert!(owner.reserve_remove(&remove).is_err());
        assert!(permit.allows("msb", "sandbox_remove", Some(&remove)));
        assert!(!permit.allows("other", "sandbox_remove", Some(&remove)));
        assert!(!permit.allows("msb", "sandbox_stop", Some(&remove)));
        assert!(!permit.allows(
            "msb",
            "sandbox_remove",
            Some(&json!({"name":"foreign"}).as_object().unwrap().clone())
        ));
        permit.removed();
        assert!(owner.reserve_remove(&remove).is_err());
    }

    #[test]
    fn creation_cannot_replace_an_existing_sandbox() {
        let owner = CleanupSession::new("msb");
        let name = "labby-tailcat-0123456789abcdef0123456789abcdef";
        for lifecycle in [json!({"replace":true}), json!({"replaceTimeoutMs":1000})] {
            let mut args = json!({"name":name,"lifecycle":lifecycle})
                .as_object()
                .unwrap()
                .clone();
            assert!(owner.prepare_create(&mut args).is_err());
        }
    }

    #[test]
    fn undispatched_cleanup_reservation_is_retryable() {
        let owner = CleanupSession::new("msb");
        let name = "labby-tailcat-0123456789abcdef0123456789abcdef";
        let mut args = json!({"name":name}).as_object().unwrap().clone();
        owner.prepare_create(&mut args).unwrap();
        owner.finish_create(name, true);
        let remove = json!({"name":name}).as_object().unwrap().clone();
        drop(owner.reserve_remove(&remove).unwrap());
        assert!(owner.reserve_remove(&remove).is_ok());
    }

    #[test]
    fn dispatched_cleanup_with_uncertain_result_is_not_replayed() {
        let owner = CleanupSession::new("msb");
        let name = "labby-tailcat-0123456789abcdef0123456789abcdef";
        let mut args = json!({"name":name}).as_object().unwrap().clone();
        owner.prepare_create(&mut args).unwrap();
        owner.finish_create(name, true);
        let remove = json!({"name":name}).as_object().unwrap().clone();
        let mut permit = owner.reserve_remove(&remove).unwrap();
        permit.mark_dispatched();
        drop(permit);
        assert!(owner.reserve_remove(&remove).is_err());
    }

    #[test]
    fn cleanup_registry_has_a_hard_bound() {
        let owner = CleanupSession::new("msb");
        for i in 0..MAX_SANDBOXES {
            let mut args = json!({"name":format!("labby-tailcat-{i:032x}")})
                .as_object()
                .unwrap()
                .clone();
            owner.prepare_create(&mut args).unwrap();
        }
        let mut extra = json!({"name":"labby-tailcat-ffffffffffffffffffffffffffffffff"})
            .as_object()
            .unwrap()
            .clone();
        assert!(owner.prepare_create(&mut extra).is_err());
    }

    #[test]
    fn cleanup_rejects_arbitrary_names_and_forged_ownership() {
        let owner = CleanupSession::new("msb");
        let mut args = json!({"name":"existing-user-vm"})
            .as_object()
            .unwrap()
            .clone();
        assert!(owner.prepare_create(&mut args).is_err());
        assert!(!owner.matches_labels(&json!({"labby-tailcat-owner":"forged"})));
    }
}
