//! Experimental, operator-enabled disposable workloads for Code Mode.
//!
//! This is separate from the existing Microsandbox *runner transport*. The
//! operator pins one image and resource ceiling; snippets cannot grant mounts,
//! networking, credentials, or change that profile.
use crate::error::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[cfg(feature = "microsandbox-sdk")]
mod sdk;

/// Operator-owned spike profile. No credentials or host paths are accepted.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfile {
    /// Exact cached immutable OCI reference allowed by the operator.
    pub image: String,
    /// vCPU ceiling, 1–2.
    pub cpus: u8,
    /// Memory ceiling in MiB, 128–1024.
    pub memory_mib: u32,
    /// Total workload (including projection) budget, 1–15000 ms.
    pub timeout_ms: u64,
}

/// One request; files are caller-supplied text projected into the guest.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRun {
    /// Must exactly match the operator-owned image.
    pub image: String,
    /// Executable followed by arguments, without implicit shell interpretation.
    pub command: Vec<String>,
    /// Optional lower timeout; cannot raise the operator ceiling.
    pub timeout_ms: Option<u64>,
    /// At most 16 flat relative filenames, 128 KiB combined, under `/work`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

pub(super) fn error(kind: &str, message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: kind.to_string(),
        message: message.into(),
    }
}

impl SandboxProfile {
    /// Validate operator resource and image limits before any runtime activity.
    pub fn validate(&self) -> Result<(), ToolError> {
        let Some((reference, digest)) = self.image.rsplit_once("@sha256:") else {
            return Err(error(
                "invalid_param",
                "profile image must be an immutable OCI sha256 reference",
            ));
        };
        if reference.is_empty()
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(error(
                "invalid_param",
                "profile image has an invalid digest",
            ));
        }
        if !(1..=2).contains(&self.cpus)
            || !(128..=1024).contains(&self.memory_mib)
            || !(1..=15000).contains(&self.timeout_ms)
        {
            return Err(error(
                "invalid_param",
                "profile exceeds spike resource limits",
            ));
        }
        Ok(())
    }
}

impl SandboxRun {
    /// Validate the request against its operator profile without launching a VM.
    pub fn validate(&self, profile: &SandboxProfile) -> Result<(), ToolError> {
        profile.validate()?;
        if self.image != profile.image {
            return Err(error("forbidden", "image does not match operator profile"));
        }
        if self.command.is_empty()
            || self.command.len() > 64
            || self.command[0].is_empty()
            || self.command.iter().any(|s| s.contains('\0'))
            || self.command.iter().map(String::len).sum::<usize>() > 8192
        {
            return Err(error(
                "invalid_param",
                "command must contain a bounded executable and argument list",
            ));
        }
        if self
            .timeout_ms
            .is_some_and(|ms| ms == 0 || ms > profile.timeout_ms)
        {
            return Err(error("invalid_param", "timeout exceeds operator profile"));
        }
        if self.files.len() > 16 || self.files.values().map(String::len).sum::<usize>() > 128 * 1024
        {
            return Err(error(
                "invalid_param",
                "projected files exceed spike limits",
            ));
        }
        if self.files.keys().any(|p| {
            p.is_empty() || p.len() > 128 || p == "." || p == ".." || p.contains(['/', '\\', '\0'])
        }) {
            return Err(error(
                "invalid_param",
                "projected files must have flat relative filenames",
            ));
        }
        Ok(())
    }
}

pub(crate) fn javascript() -> &'static str {
    r#"
globalThis.codemode = globalThis.codemode || {};
codemode.sandbox = Object.freeze({
  run: function(request) { return callTool("sandbox::run", request); }
});
"#
}

pub(crate) async fn dispatch(method: &str, params: Value) -> Result<Value, ToolError> {
    if method != "run" {
        return Err(error("not_found", "unknown sandbox method"));
    }
    #[cfg(feature = "microsandbox-sdk")]
    {
        let raw = std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").map_err(|_| {
            error(
                "forbidden",
                "sandbox workloads require an operator execution profile",
            )
        })?;
        let profile: SandboxProfile = serde_json::from_str(&raw)
            .map_err(|_| error("invalid_param", "invalid operator sandbox profile"))?;
        let request: SandboxRun = serde_json::from_value(params)
            .map_err(|_| error("invalid_param", "invalid sandbox run request"))?;
        sdk::run(profile, request).await
    }
    #[cfg(not(feature = "microsandbox-sdk"))]
    {
        drop(params);
        Err(error(
            "not_available",
            "build with microsandbox-sdk to enable sandbox workloads",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> SandboxProfile {
        SandboxProfile {
            image: format!("example/python@sha256:{}", "a".repeat(64)),
            cpus: 1,
            memory_mib: 256,
            timeout_ms: 1000,
        }
    }
    fn request() -> SandboxRun {
        SandboxRun {
            image: profile().image,
            command: vec!["python".into(), "/work/analyze.py".into()],
            timeout_ms: Some(500),
            files: BTreeMap::from([("analyze.py".into(), "print(42)".into())]),
        }
    }
    #[test]
    fn sandbox_is_reserved_and_requires_unscoped_local_authority() {
        use crate::local_provider::{LocalProviderName, try_parse_local_provider_call};
        let call = try_parse_local_provider_call("sandbox::run")
            .expect("parse")
            .expect("local call");
        assert_eq!(call.provider, LocalProviderName::Sandbox);
        assert!(try_parse_local_provider_call("sandbox::").is_err());
        assert!(crate::execute::local_providers_allowed(
            &crate::CodeModeCaller::TrustedLocal,
            &crate::ToolScope::default()
        ));
        assert!(!crate::execute::local_providers_allowed(
            &crate::CodeModeCaller::TrustedLocal,
            &crate::ToolScope::default().read_only()
        ));
    }

    #[test]
    fn profile_and_projection_are_bounded() {
        let p = profile();
        let mut r = request();
        assert!(r.validate(&p).is_ok());
        for path in ["../token", "/home/token", "a/b", "a\\b", ".", ".."] {
            r.files = BTreeMap::from([(path.into(), "bad".into())]);
            assert!(r.validate(&p).is_err());
        }
        r = request();
        r.timeout_ms = Some(1001);
        assert!(r.validate(&p).is_err());
        r = request();
        r.image = "python:latest".into();
        assert!(r.validate(&p).is_err());
        r = request();
        r.files.insert("large".into(), "x".repeat(128 * 1024));
        assert!(r.validate(&p).is_err());
    }
    #[test]
    fn caller_cannot_project_credentials_or_network_policy() {
        assert!(
            serde_json::from_value::<SandboxRun>(
                serde_json::json!({"image":profile().image,"command":["python"],"mounts":["/home"]})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SandboxRun>(
                serde_json::json!({"image":profile().image,"command":["python"],"network":true})
            )
            .is_err()
        );
    }
    #[test]
    fn mutable_or_invalid_profiles_are_rejected() {
        let mut p = profile();
        p.image = "python:latest".into();
        assert!(p.validate().is_err());
        p = profile();
        p.cpus = 3;
        assert!(p.validate().is_err());
    }
    #[test]
    fn javascript_bridge_is_executable() {
        let runtime = javy::Runtime::new(javy::Config::default()).expect("runtime");
        runtime.context().with(|ctx| {
            ctx.eval::<(), _>("globalThis.callTool=(id,p)=>({id,p});")
                .expect("stub");
            ctx.eval::<(), _>(javascript()).expect("shim");
            let id: String = ctx
                .eval("codemode.sandbox.run({image:'x'}).id")
                .expect("call");
            assert_eq!(id, "sandbox::run");
        });
    }
}
