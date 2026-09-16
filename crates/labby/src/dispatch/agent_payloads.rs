//! Content-addressed immutable payloads for LLM-backed Agents and Agent Tasks.

use std::path::{Path, PathBuf};

use labby_primitives::digest::Sha256Digest;
use serde::{Deserialize, Serialize};

use crate::{
    access::AccessStore,
    dispatch::{error::ToolError, fs_atomic::write_bytes_atomic},
};

pub(crate) const DEFAULT_MODEL: &str = "chatgpt-browser";
pub(crate) const MAX_AGENT_INSTRUCTIONS_BYTES: usize = 32 * 1024;
pub(crate) const MAX_TASK_INPUT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentLlmPayload {
    pub version: u32,
    pub model: String,
    pub instructions: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentPayloadStore {
    root: PathBuf,
}

impl AgentPayloadStore {
    pub(crate) fn for_access_store(store: &AccessStore) -> Self {
        Self::new(store.storage_dir().join("agent-payloads/sha256"))
    }

    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn store_agent(
        &self,
        model: Option<&str>,
        instructions: &str,
        expected_digest: Option<&str>,
    ) -> Result<String, ToolError> {
        let model = model.unwrap_or(DEFAULT_MODEL).trim();
        validate_model(model)?;
        validate_text(
            instructions,
            MAX_AGENT_INSTRUCTIONS_BYTES,
            "instructions",
            false,
        )?;
        let payload = AgentLlmPayload {
            version: 1,
            model: model.to_owned(),
            instructions: instructions.to_owned(),
        };
        let bytes = serde_json::to_vec(&payload).map_err(internal)?;
        self.store_bytes(&bytes, expected_digest, "content_digest")
    }

    pub(crate) fn load_agent(&self, digest: &str) -> Result<AgentLlmPayload, ToolError> {
        let bytes = self.load_bytes(digest, MAX_AGENT_INSTRUCTIONS_BYTES + 1024)?;
        let payload: AgentLlmPayload =
            serde_json::from_slice(&bytes).map_err(|_| ToolError::Sdk {
                sdk_kind: "protocol_error".into(),
                message: "Agent content is not a Labby LLM Agent payload".into(),
            })?;
        if payload.version != 1 {
            return Err(ToolError::Sdk {
                sdk_kind: "protocol_error".into(),
                message: "Agent LLM payload version is not supported".into(),
            });
        }
        validate_model(&payload.model)?;
        validate_text(
            &payload.instructions,
            MAX_AGENT_INSTRUCTIONS_BYTES,
            "instructions",
            false,
        )?;
        Ok(payload)
    }

    pub(crate) fn store_task_input(
        &self,
        input: &str,
        expected_digest: Option<&str>,
    ) -> Result<String, ToolError> {
        validate_text(input, MAX_TASK_INPUT_BYTES, "input", false)?;
        self.store_bytes(input.as_bytes(), expected_digest, "input_digest")
    }

    pub(crate) fn load_task_input(&self, digest: &str) -> Result<String, ToolError> {
        let bytes = self.load_bytes(digest, MAX_TASK_INPUT_BYTES)?;
        String::from_utf8(bytes).map_err(|_| ToolError::Sdk {
            sdk_kind: "protocol_error".into(),
            message: "Agent Task input is not UTF-8".into(),
        })
    }

    pub(crate) fn store_output(&self, output: &str) -> Result<String, ToolError> {
        validate_text(output, MAX_OUTPUT_BYTES, "output", true)?;
        self.store_bytes(output.as_bytes(), None, "output_digest")
    }

    pub(crate) fn load_output(&self, digest: &str) -> Result<String, ToolError> {
        let bytes = self.load_bytes(digest, MAX_OUTPUT_BYTES)?;
        String::from_utf8(bytes).map_err(|_| ToolError::Sdk {
            sdk_kind: "protocol_error".into(),
            message: "Agent output is not UTF-8".into(),
        })
    }

    fn store_bytes(
        &self,
        bytes: &[u8],
        expected_digest: Option<&str>,
        digest_param: &str,
    ) -> Result<String, ToolError> {
        let digest = Sha256Digest::of(bytes);
        if expected_digest.is_some_and(|expected| expected != digest.as_str()) {
            return Err(invalid(
                digest_param,
                format!("{digest_param} does not match the supplied content"),
            ));
        }
        self.ensure_root()?;
        let path = self.path_for(&digest);
        if path.exists() {
            let existing = std::fs::read(&path).map_err(internal)?;
            if existing != bytes {
                return Err(ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: "content-addressed Agent payload collision detected".into(),
                });
            }
            return Ok(digest.to_string());
        }
        write_bytes_atomic(&path, bytes)?;
        restrict_file(&path)?;
        Ok(digest.to_string())
    }

    fn load_bytes(&self, digest: &str, max_bytes: usize) -> Result<Vec<u8>, ToolError> {
        let digest = Sha256Digest::parse(digest.to_owned()).map_err(|_| {
            invalid(
                "digest",
                "digest must be canonical sha256:<64 lowercase hex>",
            )
        })?;
        let path = self.path_for(&digest);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ToolError::Sdk {
                    sdk_kind: "unavailable".into(),
                    message: "pinned Agent content is not materialized on this Labby".into(),
                }
            } else {
                internal(error)
            }
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: "Agent payload path is not a regular file".into(),
            });
        }
        if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > max_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "protocol_error".into(),
                message: "Agent payload exceeds its configured size bound".into(),
            });
        }
        let bytes = std::fs::read(&path).map_err(internal)?;
        if Sha256Digest::of(&bytes) != digest {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: "Agent payload digest verification failed".into(),
            });
        }
        Ok(bytes)
    }

    fn ensure_root(&self) -> Result<(), ToolError> {
        std::fs::create_dir_all(&self.root).map_err(internal)?;
        restrict_directory(&self.root)
    }

    fn path_for(&self, digest: &Sha256Digest) -> PathBuf {
        self.root.join(digest.hex())
    }
}

fn validate_model(model: &str) -> Result<(), ToolError> {
    if model.is_empty()
        || model.len() > 128
        || model.chars().any(char::is_whitespace)
        || model.chars().any(char::is_control)
    {
        return Err(invalid(
            "model",
            "model must be a bounded identifier without whitespace",
        ));
    }
    Ok(())
}

fn validate_text(
    value: &str,
    max_bytes: usize,
    param: &str,
    allow_empty: bool,
) -> Result<(), ToolError> {
    if (!allow_empty && value.trim().is_empty()) || value.len() > max_bytes {
        return Err(invalid(
            param,
            format!(
                "{param} must contain {}-{} UTF-8 bytes",
                usize::from(!allow_empty),
                max_bytes
            ),
        ));
    }
    Ok(())
}

fn invalid(param: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: param.into(),
    }
}

fn internal(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: error.to_string(),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(internal)
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), ToolError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(internal)
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), ToolError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_are_content_addressed_and_verified() {
        let directory = tempfile::tempdir().unwrap();
        let store = AgentPayloadStore::new(directory.path().join("payloads"));
        let digest = store
            .store_agent(Some("chatgpt-browser"), "Summarize the input.", None)
            .unwrap();
        let payload = store.load_agent(&digest).unwrap();
        assert_eq!(payload.model, "chatgpt-browser");
        assert_eq!(payload.instructions, "Summarize the input.");
        let task_digest = store.store_task_input("hello", None).unwrap();
        assert_eq!(store.load_task_input(&task_digest).unwrap(), "hello");
        assert!(store.store_task_input("hello", Some(&digest)).is_err());
    }
}
