use std::collections::HashMap;

use labby_runtime::error::ToolError;

pub(super) fn read_env_values(
    path: &std::path::Path,
) -> Result<HashMap<String, String>, ToolError> {
    match dotenvy::from_path_iter(path) {
        Ok(iter) => iter.collect::<Result<HashMap<_, _>, _>>().map_err(|e| {
            ToolError::internal_message(format!("failed to parse env file {}: {e}", path.display()))
        }),
        Err(e) if e.not_found() => Ok(HashMap::new()),
        Err(e) => Err(ToolError::internal_message(format!(
            "failed to read env file {}: {e}",
            path.display()
        ))),
    }
}
