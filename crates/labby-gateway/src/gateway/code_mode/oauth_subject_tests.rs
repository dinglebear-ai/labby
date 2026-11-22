//! Identity selection must not upgrade subject-less network callers.

use labby_codemode::{CodeModeCaller, CodeModeCallerCapabilities};

use super::code_mode_host::oauth_subject;
use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;

fn scoped_callers(sub: Option<&str>, is_admin: bool) -> Vec<CodeModeCaller> {
    let capabilities = CodeModeCallerCapabilities {
        can_read: true,
        can_execute: true,
        can_use_snippets: true,
        is_admin,
    };
    let sub = sub.map(str::to_owned);
    let callers = vec![
        CodeModeCaller::Scoped {
            host_context: Default::default(),
            capabilities,
            sub: sub.clone(),
        },
        CodeModeCaller::Scoped {
            host_context: labby_codemode::CodeModeHostContext {
                upstream_private_token: Some("test-context".into()),
                ..Default::default()
            },

            capabilities,
            sub: sub.clone(),
        },
        CodeModeCaller::Scoped {
            host_context: labby_codemode::CodeModeHostContext {
                skill_context_token: Some("test-skills".into()),
                ..Default::default()
            },

            capabilities,
            sub: sub.clone(),
        },
        CodeModeCaller::Scoped {
            host_context: labby_codemode::CodeModeHostContext {
                provider: Some(labby_codemode::CodeModeProviderContext {
                    token: "test-provider".into(),
                    request_id: "test-request".into(),
                }),
                ..Default::default()
            },

            capabilities,
            sub: sub.clone(),
        },
        CodeModeCaller::Scoped {
            host_context: labby_codemode::CodeModeHostContext {
                skill_context_token: Some("test-skills".into()),
                provider: Some(labby_codemode::CodeModeProviderContext {
                    token: "test-provider".into(),
                    request_id: "test-request".into(),
                }),
                ..Default::default()
            },

            capabilities,
            sub,
        },
    ];
    callers
        .into_iter()
        .flat_map(|caller| {
            let wrapped = CodeModeCaller::WithAuthority {
                caller: Box::new(caller.clone()),
                authority_token: "test-authority".into(),
            };
            [caller, wrapped]
        })
        .collect()
}

#[test]
fn non_admin_callers_without_a_subject_never_inherit_shared_oauth() {
    for sub in [None, Some(""), Some(" \t\n")] {
        for caller in scoped_callers(sub, false) {
            assert_eq!(oauth_subject(&caller), None, "caller: {caller:?}");
        }
    }
}

#[test]
fn non_admin_oauth_subjects_preserve_exact_caller_identity() {
    for sub in ["alice", "bob", "opaque subject with spaces"] {
        for caller in scoped_callers(Some(sub), false) {
            assert_eq!(oauth_subject(&caller), Some(sub), "caller: {caller:?}");
        }
    }
}

#[test]
fn explicitly_trusted_local_and_admin_callers_keep_shared_oauth() {
    assert_eq!(
        oauth_subject(&CodeModeCaller::TrustedLocal),
        Some(SHARED_GATEWAY_OAUTH_SUBJECT)
    );
    for sub in [None, Some("operator")] {
        for caller in scoped_callers(sub, true) {
            assert_eq!(oauth_subject(&caller), Some(SHARED_GATEWAY_OAUTH_SUBJECT));
        }
    }
}
