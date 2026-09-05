use std::collections::BTreeSet;

use super::{CapabilityRef, ExecutionLoadoutError, MAX_MEMBERS, MAX_TEXT_BYTES};

pub(super) fn normalize_members(
    mut members: Vec<CapabilityRef>,
) -> Result<Vec<CapabilityRef>, ExecutionLoadoutError> {
    if members.len() > MAX_MEMBERS {
        return Err(ExecutionLoadoutError::LimitExceeded { limit: MAX_MEMBERS });
    }
    for member in &members {
        validate_text("provider", &member.provider)?;
        validate_text("memberId", &member.member_id)?;
        validate_text("expectedRevision", &member.expected_revision)?;
    }
    members.sort();
    let unique = members.iter().cloned().collect::<BTreeSet<_>>();
    if unique.len() != members.len() {
        return Err(ExecutionLoadoutError::Invalid {
            field: "members".into(),
            message: "duplicate provider-qualified capability reference".into(),
        });
    }
    Ok(members)
}

pub(super) fn validate_text(field: &str, value: &str) -> Result<(), ExecutionLoadoutError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(ExecutionLoadoutError::Invalid {
            field: field.into(),
            message: format!("must contain 1..={MAX_TEXT_BYTES} bytes"),
        });
    }
    Ok(())
}
