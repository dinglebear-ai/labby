//! Generate the canonical scenario schema.

use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&verify_scenario::scenario_schema())?
    );
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => print!("{rendered}"),
        [flag] if flag == "--write" => {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::fs::create_dir_all(workspace.join("schemas"))?;
            std::fs::write(workspace.join("schemas/scenario.schema.json"), &rendered)?;
        }
        _ => return Err("usage: scenario-schema [--write]".into()),
    }
    Ok(())
}
