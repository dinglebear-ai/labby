//! Generate the canonical invariant schema.

use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&verify_core::catalog_schema())?
    );
    if std::env::args().nth(1).as_deref() == Some("--write") {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = workspace.join("schemas/invariants.schema.json");
        std::fs::create_dir_all(output.parent().ok_or("missing schema parent")?)?;
        std::fs::write(output, &rendered)?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}
