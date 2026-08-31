use std::collections::BTreeSet;

use serde_json::Value;

use crate::dispatch::error::ToolError;

const MAX_AUDIT_SERVICES: usize = 64;

pub(super) fn audit_services(params: &Value) -> Result<Vec<String>, ToolError> {
    let values = params
        .get("services")
        .ok_or_else(|| ToolError::MissingParam {
            message: "missing required parameter `services`".into(),
            param: "services".into(),
        })?
        .as_array()
        .ok_or_else(|| ToolError::InvalidParam {
            message: "parameter `services` must be an array of service names".into(),
            param: "services".into(),
        })?;
    if values.is_empty() || values.len() > MAX_AUDIT_SERVICES {
        return Err(ToolError::InvalidParam {
            message: format!("parameter `services` must contain 1..={MAX_AUDIT_SERVICES} names"),
            param: "services".into(),
        });
    }
    let mut unique = BTreeSet::new();
    for value in values {
        let name = value
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ToolError::InvalidParam {
                message: "parameter `services` must contain non-empty strings".into(),
                param: "services".into(),
            })?;
        unique.insert(name.to_owned());
    }
    Ok(unique.into_iter().collect())
}
