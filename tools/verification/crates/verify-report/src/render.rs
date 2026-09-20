use std::fmt::Write;

use crate::{EvidenceLane, ValidatedReport};

/// Render canonical pretty JSON from the validated contract.
pub fn render_json(report: &ValidatedReport) -> String {
    serde_json::to_string_pretty(report.report()).expect("validated report serializes") + "\n"
}

/// Render a compact terminal report from the JSON contract.
pub fn render_text(report: &ValidatedReport) -> String {
    let value = report.report();
    let mut out = format!(
        "Project: {}  catalog: {}\nQualification: {:?}\n",
        text(&value.project),
        text(&value.catalog.id),
        report.qualification()
    );
    writeln!(out, "Invariants: {}", value.coverage.invariant_ids.len()).unwrap();
    writeln!(out, "Scenarios: {}", value.scenarios.total).unwrap();
    writeln!(
        out,
        "Replay uncovered: {}",
        join_ids(
            value
                .coverage
                .replay_uncovered_ids
                .iter()
                .map(ToString::to_string)
        )
    )
    .unwrap();
    writeln!(
        out,
        "Unconfigured backend IDs: {}",
        join_ids(
            value
                .coverage
                .unconfigured_backend_ids
                .iter()
                .map(ToString::to_string)
        )
    )
    .unwrap();
    writeln!(
        out,
        "Backend execution uncovered: {}",
        join_ids(
            value
                .coverage
                .backend_uncovered_ids
                .iter()
                .map(ToString::to_string)
        )
    )
    .unwrap();
    for lane in lanes() {
        let rows: Vec<_> = value
            .observations
            .iter()
            .filter(|row| row.lane == lane)
            .collect();
        writeln!(out, "{:?}: {}", lane, rows.len()).unwrap();
        if rows.is_empty() {
            writeln!(out, "  no evidence").unwrap();
        }
        for row in rows {
            writeln!(
                out,
                "  {} [{:?}/{:?}] {}",
                text(&row.id),
                row.requirement,
                row.verdict,
                text(row.diagnostic.as_deref().unwrap_or("-"))
            )
            .unwrap();
        }
    }
    out
}

/// Render a PR-comment-safe Markdown summary.
pub fn render_markdown(report: &ValidatedReport) -> String {
    let value = report.report();
    let mut out = format!(
        "## Verification report: {}\n\n**Qualification:** `{:?}`  \n**Catalog:** `{}`  \n**Scenarios:** {}  \n**Replay uncovered:** {}  \n**Unconfigured backend IDs:** {}  \n**Backend execution uncovered:** {}\n\n| Lane | ID | Required | Verdict | Diagnostic |\n| --- | --- | --- | --- | --- |\n",
        md(&value.project),
        report.qualification(),
        md(&value.catalog.id),
        value.scenarios.total,
        md(&join_ids(
            value
                .coverage
                .replay_uncovered_ids
                .iter()
                .map(ToString::to_string)
        )),
        md(&join_ids(
            value
                .coverage
                .unconfigured_backend_ids
                .iter()
                .map(ToString::to_string)
        )),
        md(&join_ids(
            value
                .coverage
                .backend_uncovered_ids
                .iter()
                .map(ToString::to_string)
        ))
    );
    for row in &value.observations {
        writeln!(
            out,
            "| `{:?}` | {} | `{:?}` | `{:?}` | {} |",
            row.lane,
            md(&row.id),
            row.requirement,
            row.verdict,
            md(row.diagnostic.as_deref().unwrap_or("-"))
        )
        .unwrap();
    }
    for lane in lanes()
        .into_iter()
        .filter(|lane| !value.observations.iter().any(|row| row.lane == *lane))
    {
        writeln!(
            out,
            "| `{:?}` | no evidence | `Optional` | `Skipped` | no observation supplied |",
            lane
        )
        .unwrap();
    }
    out
}

/// Render a standalone static HTML matrix.
pub fn render_html(report: &ValidatedReport) -> String {
    let value = report.report();
    let mut rows = String::new();
    for row in &value.observations {
        write!(
            rows,
            "<tr><td>{:?}</td><td>{}</td><td>{:?}</td><td>{:?}</td><td>{}</td></tr>",
            row.lane,
            html(&row.id),
            row.requirement,
            row.verdict,
            html(row.diagnostic.as_deref().unwrap_or("-"))
        )
        .unwrap();
    }
    for lane in lanes()
        .into_iter()
        .filter(|lane| !value.observations.iter().any(|row| row.lane == *lane))
    {
        write!(rows, "<tr><td>{lane:?}</td><td>no evidence</td><td>Optional</td><td>Skipped</td><td>no observation supplied</td></tr>").unwrap();
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Verification report: {0}</title><style>body{{font:16px system-ui;max-width:90rem;margin:auto;padding:2rem}}table{{border-collapse:collapse;width:100%}}th,td{{border:1px solid #888;padding:.4rem;text-align:left}}code{{overflow-wrap:anywhere}}</style></head><body><h1>Verification report: {0}</h1><p><strong>Qualification:</strong> {1:?}</p><p><strong>Catalog:</strong> <code>{2}</code></p><p><strong>Scenarios:</strong> {3}</p><p><strong>Replay uncovered:</strong> <code>{4}</code></p><p><strong>Unconfigured backend IDs:</strong> <code>{5}</code></p><p><strong>Backend execution uncovered:</strong> <code>{6}</code></p><table><thead><tr><th>Lane</th><th>ID</th><th>Required</th><th>Verdict</th><th>Diagnostic</th></tr></thead><tbody>{7}</tbody></table></body></html>\n",
        html(&value.project),
        report.qualification(),
        html(&value.catalog.id),
        value.scenarios.total,
        html(&join_ids(
            value
                .coverage
                .replay_uncovered_ids
                .iter()
                .map(ToString::to_string)
        )),
        html(&join_ids(
            value
                .coverage
                .unconfigured_backend_ids
                .iter()
                .map(ToString::to_string)
        )),
        html(&join_ids(
            value
                .coverage
                .backend_uncovered_ids
                .iter()
                .map(ToString::to_string)
        )),
        rows
    )
}

fn lanes() -> [EvidenceLane; 7] {
    [
        EvidenceLane::ModelChecking,
        EvidenceLane::ModelReplay,
        EvidenceLane::CounterexampleReproduction,
        EvidenceLane::Conformance,
        EvidenceLane::RealProcess,
        EvidenceLane::BrowserEmulation,
        EvidenceLane::ActualHost,
    ]
}
fn join_ids(values: impl Iterator<Item = String>) -> String {
    let value = values.collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        "none".into()
    } else {
        value
    }
}
fn md(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "&#96;")
        .replace('|', "\\|")
        .replace('\r', " ")
        .replace('\n', "<br>")
}
fn html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn text(value: &str) -> String {
    value.replace('\n', "\\n").replace('\t', "\\t")
}
