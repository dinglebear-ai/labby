//! Crash-recoverable two-file provider configuration transaction.
use crate::config::host_write::{HostConfigLock, HostWriteError};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub config: String,
    pub environment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub operation_id: String,
    pub version: String,
    pub committed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Prepared,
    Committed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Intent {
    phase: Phase,
    operation_id: String,
    old_config: String,
    old_environment: String,
    new_config: String,
    new_environment: String,
    old_integrity: String,
    new_integrity: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("provider configuration changed")]
    Stale,
    #[error("provider configuration recovery is required")]
    RecoveryRequired,
    #[error("provider configuration is busy")]
    Busy,
    #[error("provider configuration transaction is invalid")]
    Invalid,
    #[error("provider configuration durability failed")]
    Durability,
}

pub struct Store {
    config: PathBuf,
    environment: PathBuf,
    state: PathBuf,
}

impl Store {
    pub fn new(config: PathBuf, environment: PathBuf, state: PathBuf) -> Self {
        Self {
            config,
            environment,
            state,
        }
    }

    pub fn current_version(&self) -> Result<String, StoreError> {
        let key = self.key()?;
        let config = HostConfigLock::acquire(&self.config)
            .map_err(map_host)?
            .read_raw()
            .map_err(map_host)?;
        let environment = HostConfigLock::acquire(&self.environment)
            .map_err(map_host)?
            .read_raw()
            .map_err(map_host)?;
        digest(
            &key,
            &Pair {
                config,
                environment,
            },
        )
    }

    pub fn recover(&self) -> Result<Option<Outcome>, StoreError> {
        let config = HostConfigLock::acquire(&self.config).map_err(map_host)?;
        let environment = HostConfigLock::acquire(&self.environment).map_err(map_host)?;
        self.recover_locked(&config, &environment)
    }

    pub fn commit(
        &self,
        operation: &str,
        expected_version: &str,
        pair: &Pair,
    ) -> Result<Outcome, StoreError> {
        validate_operation(operation)?;
        if pair.config.len() > 8 * 1024 * 1024 || pair.environment.len() > 8 * 1024 * 1024 {
            return Err(StoreError::Invalid);
        }
        std::fs::create_dir_all(&self.state).map_err(|_| StoreError::Durability)?;
        let key = self.key()?;
        let config = HostConfigLock::acquire(&self.config).map_err(map_host)?;
        let environment = HostConfigLock::acquire(&self.environment).map_err(map_host)?;
        if let Some(outcome) = self.recover_locked(&config, &environment)?
            && outcome.operation_id == operation
        {
            if outcome.version == digest(&key, pair)? {
                return Ok(outcome);
            }
            return Err(StoreError::Invalid);
        }
        let old = Pair {
            config: config.read_raw().map_err(map_host)?,
            environment: environment.read_raw().map_err(map_host)?,
        };
        if digest(&key, &old)? != expected_version {
            return Err(StoreError::Stale);
        }
        let names = SnapshotNames::new(operation);
        self.write_state(&names.old_config, &old.config)?;
        self.write_state(&names.old_environment, &old.environment)?;
        self.write_state(&names.new_config, &pair.config)?;
        self.write_state(&names.new_environment, &pair.environment)?;
        let mut intent = Intent {
            phase: Phase::Prepared,
            operation_id: operation.into(),
            old_config: names.old_config,
            old_environment: names.old_environment,
            new_config: names.new_config,
            new_environment: names.new_environment,
            old_integrity: digest(&key, &old)?,
            new_integrity: digest(&key, pair)?,
        };
        self.write_intent(&intent)?;
        config.write(&pair.config).map_err(map_host)?;
        environment.write(&pair.environment).map_err(map_host)?;
        intent.phase = Phase::Committed;
        self.write_intent(&intent)?;
        Ok(Outcome {
            operation_id: operation.into(),
            version: intent.new_integrity,
            committed: true,
        })
    }

    fn recover_locked(
        &self,
        config: &HostConfigLock,
        environment: &HostConfigLock,
    ) -> Result<Option<Outcome>, StoreError> {
        let Some(intent) = self.read_intent()? else {
            return Ok(None);
        };
        validate_operation(&intent.operation_id)?;
        let key = self.key()?;
        let pair = match intent.phase {
            Phase::Prepared => Pair {
                config: self.read_state(&intent.old_config)?,
                environment: self.read_state(&intent.old_environment)?,
            },
            Phase::Committed => Pair {
                config: self.read_state(&intent.new_config)?,
                environment: self.read_state(&intent.new_environment)?,
            },
        };
        let expected = match intent.phase {
            Phase::Prepared => &intent.old_integrity,
            Phase::Committed => &intent.new_integrity,
        };
        if digest(&key, &pair)?.as_str() != expected {
            return Err(StoreError::RecoveryRequired);
        }
        config.write(&pair.config).map_err(map_host)?;
        environment.write(&pair.environment).map_err(map_host)?;
        Ok((intent.phase == Phase::Committed).then_some(Outcome {
            operation_id: intent.operation_id,
            version: intent.new_integrity,
            committed: true,
        }))
    }

    fn key(&self) -> Result<[u8; 32], StoreError> {
        std::fs::create_dir_all(&self.state).map_err(|_| StoreError::Durability)?;
        let path = self.state.join("integrity.key");
        let lock = HostConfigLock::acquire(&path).map_err(map_host)?;
        let raw = lock.read_raw().map_err(map_host)?;
        if raw.is_empty() {
            let mut key = [0_u8; 32];
            getrandom::fill(&mut key).map_err(|_| StoreError::Durability)?;
            lock.write(&URL_SAFE_NO_PAD.encode(key)).map_err(map_host)?;
            Ok(key)
        } else {
            let decoded = URL_SAFE_NO_PAD
                .decode(raw)
                .map_err(|_| StoreError::RecoveryRequired)?;
            decoded.try_into().map_err(|_| StoreError::RecoveryRequired)
        }
    }

    fn intent_path(&self) -> PathBuf {
        self.state.join("active.json")
    }
    fn write_intent(&self, intent: &Intent) -> Result<(), StoreError> {
        let raw = serde_json::to_string(intent).map_err(|_| StoreError::Invalid)?;
        HostConfigLock::acquire(&self.intent_path())
            .map_err(map_host)?
            .write(&raw)
            .map_err(map_host)
    }
    fn read_intent(&self) -> Result<Option<Intent>, StoreError> {
        let raw = HostConfigLock::acquire(&self.intent_path())
            .map_err(map_host)?
            .read_raw()
            .map_err(map_host)?;
        if raw.is_empty() {
            Ok(None)
        } else {
            serde_json::from_str(&raw)
                .map(Some)
                .map_err(|_| StoreError::RecoveryRequired)
        }
    }
    fn write_state(&self, name: &str, raw: &str) -> Result<(), StoreError> {
        HostConfigLock::acquire(&self.state.join(name))
            .map_err(map_host)?
            .write(raw)
            .map_err(map_host)
    }
    fn read_state(&self, name: &str) -> Result<String, StoreError> {
        if Path::new(name).components().count() != 1 {
            return Err(StoreError::RecoveryRequired);
        }
        HostConfigLock::acquire(&self.state.join(name))
            .map_err(map_host)?
            .read_raw()
            .map_err(map_host)
    }
}

struct SnapshotNames {
    old_config: String,
    old_environment: String,
    new_config: String,
    new_environment: String,
}
impl SnapshotNames {
    fn new(operation: &str) -> Self {
        Self {
            old_config: format!("{operation}.config.old"),
            old_environment: format!("{operation}.env.old"),
            new_config: format!("{operation}.config.new"),
            new_environment: format!("{operation}.env.new"),
        }
    }
}

fn validate_operation(value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(StoreError::Invalid)
    } else {
        Ok(())
    }
}
fn digest(key: &[u8; 32], pair: &Pair) -> Result<String, StoreError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| StoreError::Durability)?;
    mac.update(&(pair.config.len() as u64).to_be_bytes());
    mac.update(pair.config.as_bytes());
    mac.update(&(pair.environment.len() as u64).to_be_bytes());
    mac.update(pair.environment.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}
fn map_host(error: HostWriteError) -> StoreError {
    match error {
        HostWriteError::Busy => StoreError::Busy,
        HostWriteError::InvalidDocument | HostWriteError::TooLarge | HostWriteError::UnsafePath => {
            StoreError::Invalid
        }
        HostWriteError::Io | HostWriteError::Durability => StoreError::Durability,
    }
}
