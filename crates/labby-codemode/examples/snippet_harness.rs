//! Offline developer entry point for the production snippet fixture harness.
use std::error::Error;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use labby_codemode::snippet::harness::{MAX_FIXTURE_BYTES, SnippetFixture, run_fixture};
use labby_codemode::snippet::store::{
    ResolvedSnippet, SnippetSource, frontmatter, validate_snippet_body,
};

fn read_bounded(path: &Path, limit: usize) -> Result<String, Box<dyn Error>> {
    let mut data = String::new();
    std::fs::File::open(path)?
        .take((limit + 1) as u64)
        .read_to_string(&mut data)?;
    if data.len() > limit {
        return Err(format!("{} exceeds the input limit", path.display()).into());
    }
    Ok(data)
}

async fn run(args: &[String]) -> Result<bool, Box<dyn Error>> {
    if args == ["--schema"] {
        println!(
            "{}",
            serde_json::to_string_pretty(&schemars::schema_for!(SnippetFixture))?
        );
        return Ok(true);
    }
    if !(2..=3).contains(&args.len()) {
        return Err(
            "usage: snippet_harness SOURCE.md FIXTURE.json [INPUT_JSON], or --schema".into(),
        );
    }
    let path = PathBuf::from(&args[0]);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("source has no valid file stem")?
        .to_string();
    let body = read_bounded(&path, 2 * labby_codemode::MAX_SOURCE_BYTES)?;
    validate_snippet_body(&name, &body)?;
    let metadata = frontmatter(&body)?;
    let snippet = ResolvedSnippet {
        name,
        description: metadata.as_ref().map(|m| m.description.clone()),
        tags: metadata
            .as_ref()
            .map(|m| m.tags.clone())
            .unwrap_or_default(),
        inputs: metadata
            .as_ref()
            .map(|m| m.inputs.clone())
            .unwrap_or_default(),
        tools: metadata.and_then(|m| m.tools),
        source: SnippetSource::User,
        path,
        body,
    };
    let fixture: SnippetFixture =
        serde_json::from_str(&read_bounded(Path::new(&args[1]), MAX_FIXTURE_BYTES)?)?;
    let input = serde_json::from_str(args.get(2).map_or("{}", String::as_str))?;
    let report = run_fixture(&snippet, input, &fixture).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(report.passed)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["internal", "code-mode-runner"] {
        return labby_codemode::run_code_mode_runner_stdio();
    }
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| -> Box<dyn Error> { Box::new(e) })
        .and_then(|runtime| runtime.block_on(run(&args)));
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
