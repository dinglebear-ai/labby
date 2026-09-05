//! Confidential, bounded replay state for federated Depot discovery.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::collections::{BTreeMap, VecDeque};
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const IDLE_TTL: Duration = Duration::from_mins(15);
const ABSOLUTE_TTL: Duration = Duration::from_hours(1);
const MAX_CHAINS_PER_ACTOR: usize = 8;
const MAX_CHAINS: usize = 128;
const MAX_ACTOR_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHAIN_BYTES: usize = 4 * 1024 * 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const RETAINED_TRANSITIONS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub actor: String,
    pub authority_epoch: String,
    pub scope: String,
    pub query: String,
    pub page_contract: String,
    pub registry_epoch: String,
    pub providers: Vec<(String, String, String)>,
}

impl Binding {
    pub async fn for_browser(
        authority: &labby_auth::browser_authority::BrowserAuthority,
        required_scope: &str,
        scope: String,
        query: String,
        page_contract: String,
        registry_epoch: String,
        providers: Vec<(String, String, String)>,
    ) -> Result<Self, CursorError> {
        let grant = authority
            .revalidate()
            .await
            .map_err(|_| CursorError::Expired)?;
        if !grant.has_scope(required_scope) {
            return Err(CursorError::Expired);
        }
        Ok(Self {
            actor: authority.actor_key(),
            authority_epoch: authority.public_epoch(),
            scope,
            query,
            page_contract,
            registry_epoch,
            providers,
        })
    }

    fn valid(&self) -> bool {
        !self.actor.is_empty()
            && self.actor.len() <= 256
            && !self.authority_epoch.is_empty()
            && self.authority_epoch.len() <= 256
            && self.scope.len() <= 256
            && self.query.len() <= 200
            && self.page_contract.len() <= 128
            && self.registry_epoch.len() <= 256
            && self.providers.len() <= 16
            && self.providers.iter().all(|(id, incarnation, listing)| {
                !id.is_empty() && id.len() <= 64 && incarnation.len() <= 128 && listing.len() <= 256
            })
    }

    fn charge(&self) -> usize {
        512 + self.actor.capacity()
            + self.authority_epoch.capacity()
            + self.scope.capacity()
            + self.query.capacity()
            + self.page_contract.capacity()
            + self.registry_epoch.capacity()
            + self.providers.capacity() * size_of::<(String, String, String)>()
            + self
                .providers
                .iter()
                .map(|(a, b, c)| a.capacity() + b.capacity() + c.capacity())
                .sum::<usize>()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub response: Vec<u8>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("cursor expired; restart discovery")]
    Expired,
    #[error("cursor capacity exhausted")]
    Capacity,
    #[error("cursor state is invalid")]
    Invalid,
}

pub enum PageInput {
    Replay(Page),
    Compute(Lease),
}

impl std::fmt::Debug for PageInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Replay(_) => f.write_str("Replay(<redacted>)"),
            Self::Compute(_) => f.write_str("Compute(<redacted>)"),
        }
    }
}

impl PageInput {
    pub fn replay(self) -> Option<Page> {
        match self {
            Self::Replay(page) => Some(page),
            Self::Compute(_) => None,
        }
    }
}

struct Transition {
    handle: String,
    state: Vec<u8>,
    page: Option<Page>,
    computing: bool,
    notify: Arc<Notify>,
}

impl Transition {
    fn charge(&self) -> usize {
        256 + self.handle.capacity()
            + self.state.capacity()
            + self.page.as_ref().map_or(0, |page| {
                page.response.capacity() + page.next_cursor.as_ref().map_or(0, String::capacity)
            })
    }
}

struct Chain {
    binding: Binding,
    created: Instant,
    last_access: Instant,
    transitions: VecDeque<Transition>,
}

impl Chain {
    fn charge(&self) -> usize {
        self.binding.charge()
            + self.transitions.capacity() * size_of::<Transition>()
            + self
                .transitions
                .iter()
                .map(Transition::charge)
                .sum::<usize>()
    }
    fn active(&self) -> bool {
        self.transitions.iter().any(|item| item.computing)
    }
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) >= ABSOLUTE_TTL
            || now.saturating_duration_since(self.last_access) >= IDLE_TTL
    }
}

#[derive(Default)]
struct State {
    chains: BTreeMap<u64, Chain>,
    next_id: u64,
}

#[derive(Clone, Default)]
pub struct CursorStore {
    inner: Arc<Mutex<State>>,
}

impl CursorStore {
    pub fn purge_provider(&self, provider: &str, incarnation: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.chains.retain(|_, chain| {
            !chain
                .binding
                .providers
                .iter()
                .any(|(id, current, _)| id == provider && current == incarnation)
        });
    }

    pub async fn create(
        &self,
        binding: Binding,
        state: Vec<u8>,
        now: Instant,
    ) -> Result<String, CursorError> {
        if !binding.valid() || state.len() > MAX_CHAIN_BYTES {
            return Err(CursorError::Invalid);
        }
        let handle = random_handle()?;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        purge_expired(&mut inner, now);
        evict_for_limits(
            &mut inner,
            &binding.actor,
            binding.charge() + state.capacity() + 512,
        )?;
        inner.next_id = inner.next_id.wrapping_add(1);
        let id = inner.next_id;
        inner.chains.insert(
            id,
            Chain {
                binding,
                created: now,
                last_access: now,
                transitions: VecDeque::from([Transition {
                    handle: handle.clone(),
                    state,
                    page: None,
                    computing: false,
                    notify: Arc::new(Notify::new()),
                }]),
            },
        );
        Ok(handle)
    }

    pub async fn begin(
        &self,
        handle: &str,
        binding: &Binding,
        now: Instant,
    ) -> Result<PageInput, CursorError> {
        if !valid_handle(handle) || !binding.valid() {
            return Err(CursorError::Expired);
        }
        loop {
            let notified = {
                let mut inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                purge_expired(&mut inner, now);
                let Some(chain) = inner
                    .chains
                    .values_mut()
                    .find(|chain| chain.transitions.iter().any(|item| item.handle == handle))
                else {
                    return Err(CursorError::Expired);
                };
                if &chain.binding != binding {
                    return Err(CursorError::Expired);
                }
                chain.last_access = now;
                let transition = chain
                    .transitions
                    .iter_mut()
                    .find(|item| item.handle == handle)
                    .expect("located above");
                if let Some(page) = &transition.page {
                    return Ok(PageInput::Replay(page.clone()));
                }
                if transition.computing {
                    Some(transition.notify.clone())
                } else {
                    transition.computing = true;
                    return Ok(PageInput::Compute(Lease {
                        inner: self.inner.clone(),
                        handle: handle.to_owned(),
                        state: transition.state.clone(),
                        completed: false,
                    }));
                }
            };
            if let Some(notify) = notified {
                // The timeout also closes Notify's registration race for callers
                // arriving exactly as a computation publishes its result.
                let _ = tokio::time::timeout(Duration::from_millis(25), notify.notified()).await;
            }
        }
    }
}

pub struct Lease {
    inner: Arc<Mutex<State>>,
    handle: String,
    state: Vec<u8>,
    completed: bool,
}

impl Lease {
    pub fn state(&self) -> &[u8] {
        &self.state
    }

    pub async fn complete(
        mut self,
        response: Vec<u8>,
        next_state: Option<Vec<u8>>,
        now: Instant,
    ) -> Result<Page, CursorError> {
        if response.len() > MAX_CHAIN_BYTES
            || next_state
                .as_ref()
                .is_some_and(|value| value.len() > MAX_CHAIN_BYTES)
        {
            self.release();
            return Err(CursorError::Capacity);
        }
        let next_cursor = next_state.as_ref().map(|_| random_handle()).transpose()?;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(chain_id) = inner
            .chains
            .iter()
            .find(|(_, chain)| {
                chain
                    .transitions
                    .iter()
                    .any(|item| item.handle == self.handle)
            })
            .map(|(id, _)| *id)
        else {
            self.completed = true;
            return Err(CursorError::Expired);
        };
        let chain = inner.chains.get(&chain_id).expect("located above");
        let old_charge = chain.charge();
        let actor = chain.binding.actor.clone();
        let projected = old_charge
            + response.capacity()
            + next_state.as_ref().map_or(0, Vec::capacity)
            + next_cursor.as_ref().map_or(0, String::capacity)
            + 512;
        if projected > MAX_CHAIN_BYTES
            || total_bytes(&inner) - old_charge + projected > MAX_BYTES
            || actor_bytes(&inner, &actor) - old_charge + projected > MAX_ACTOR_BYTES
        {
            drop(inner);
            self.release();
            return Err(CursorError::Capacity);
        }
        let chain = inner.chains.get_mut(&chain_id).expect("located above");
        chain.last_access = now;
        let transition = chain
            .transitions
            .iter_mut()
            .find(|item| item.handle == self.handle)
            .expect("located above");
        let page = Page {
            response,
            next_cursor: next_cursor.clone(),
        };
        transition.page = Some(page.clone());
        transition.computing = false;
        transition.notify.notify_waiters();
        transition.notify.notify_one();
        if let (Some(handle), Some(state)) = (next_cursor, next_state) {
            chain.transitions.push_back(Transition {
                handle,
                state,
                page: None,
                computing: false,
                notify: Arc::new(Notify::new()),
            });
        }
        while chain
            .transitions
            .iter()
            .filter(|item| item.page.is_some())
            .count()
            > RETAINED_TRANSITIONS
        {
            if chain.transitions.front().is_some_and(|item| item.computing) {
                break;
            }
            chain.transitions.pop_front();
        }
        self.completed = true;
        Ok(page)
    }

    fn release(&mut self) {
        if self.completed {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(item) = inner
            .chains
            .values_mut()
            .flat_map(|chain| chain.transitions.iter_mut())
            .find(|item| item.handle == self.handle)
        {
            item.computing = false;
            item.notify.notify_waiters();
            item.notify.notify_one();
        }
        self.completed = true;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.release();
    }
}

fn random_handle() -> Result<String, CursorError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| CursorError::Capacity)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn valid_handle(handle: &str) -> bool {
    handle.len() == 43
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn total_bytes(state: &State) -> usize {
    state.chains.values().map(Chain::charge).sum()
}
fn actor_bytes(state: &State, actor: &str) -> usize {
    state
        .chains
        .values()
        .filter(|chain| chain.binding.actor == actor)
        .map(Chain::charge)
        .sum()
}
fn purge_expired(state: &mut State, now: Instant) {
    state
        .chains
        .retain(|_, chain| chain.active() || !chain.expired(now));
}
fn evict_for_limits(state: &mut State, actor: &str, incoming: usize) -> Result<(), CursorError> {
    loop {
        let actor_count = state
            .chains
            .values()
            .filter(|chain| chain.binding.actor == actor)
            .count();
        if state.chains.len() < MAX_CHAINS
            && actor_count < MAX_CHAINS_PER_ACTOR
            && total_bytes(state) + incoming <= MAX_BYTES
            && actor_bytes(state, actor) + incoming <= MAX_ACTOR_BYTES
        {
            return Ok(());
        }
        let candidate = state
            .chains
            .iter()
            .filter(|(_, chain)| {
                !chain.active()
                    && (chain.binding.actor == actor
                        || state.chains.len() >= MAX_CHAINS
                        || total_bytes(state) + incoming > MAX_BYTES)
            })
            .min_by_key(|(_, chain)| chain.last_access)
            .map(|(id, _)| *id);
        let Some(id) = candidate else {
            return Err(CursorError::Capacity);
        };
        state.chains.remove(&id);
    }
}
