//! `SKILL.md` frontmatter validation and entry-vs-file comparison.
//!
//! SEP-2640 delegates the skill format entirely to the [Agent Skills
//! specification](https://agentskills.io/specification) and carries frontmatter
//! "verbatim as a JSON object — every field the author wrote, not a curated
//! subset". Two consequences shape this module:
//!
//! - Validation checks the fields the Agent Skills spec constrains and leaves
//!   every other key untouched, so a future spec revision that adds a field
//!   flows through without a change here.
//! - Comparison is structural over the whole object, not over a curated struct.
//!   The SEP requires that after fetching a `SKILL.md` for which a host holds an
//!   entry, the host "MUST parse its YAML frontmatter and compare it
//!   field-by-field against the entry's `frontmatter`", treating any
//!   discrepancy as a verification failure. A curated struct would silently drop
//!   the very fields an attacker would add.
//!
//! The `labby-codemode` snippet store hand-rolls a line-based frontmatter
//! reader; it is intentionally not reused here because it yields a fixed set of
//! fields and cannot represent arbitrary authored YAML.

use serde_json::{Map, Value};

use crate::error::ToolError;
use crate::skills::limits::{
    MAX_COMPATIBILITY_CHARS, MAX_DESCRIPTION_CHARS, MAX_FRONTMATTER_BYTES, MAX_NAME_CHARS,
    MAX_SKILL_MD_FRONTMATTER_BYTES,
};

/// Reverse-domain prefix reserved inside `metadata` for MCP extensions.
///
/// Note the dot before `skills` — this differs from the extension *capability*
/// key `io.modelcontextprotocol/skills`, which uses a slash.
pub const RESERVED_METADATA_PREFIX: &str = "io.modelcontextprotocol/";

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "validation_failed".to_string(),
        message: message.into(),
    }
}

/// True when `name` satisfies the Agent Skills naming rules: lowercase ASCII
/// letters, digits and hyphens; at most 64 characters; no leading or trailing
/// hyphen; no consecutive hyphens.
#[must_use]
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validate a skill entry's `frontmatter` object.
///
/// `expected_name`, when supplied, is the final segment of the skill path; the
/// SEP requires the two to be equal, which is what makes a skill's name
/// recoverable from its URI without a fetch.
///
/// Unrecognized keys are accepted and left alone (forward compatibility). Keys
/// under the reserved `metadata` prefix that this implementation does not know
/// are ignored rather than rejected, per the SEP.
pub fn validate_frontmatter(
    frontmatter: &Map<String, Value>,
    expected_name: Option<&str>,
) -> Result<(), ToolError> {
    let encoded = serde_json::to_vec(frontmatter)
        .map_err(|err| invalid(format!("frontmatter is not serializable: {err}")))?;
    if encoded.len() > MAX_FRONTMATTER_BYTES {
        return Err(invalid(format!(
            "frontmatter exceeds {MAX_FRONTMATTER_BYTES} bytes"
        )));
    }

    let name = frontmatter
        .get("name")
        .ok_or_else(|| invalid("frontmatter is missing the required `name` field"))?
        .as_str()
        .ok_or_else(|| invalid("frontmatter `name` must be a string"))?;
    if !is_valid_skill_name(name) {
        return Err(invalid(format!(
            "frontmatter `name` must be at most {MAX_NAME_CHARS} lowercase alphanumeric \
             or hyphen characters, with no leading, trailing, or consecutive hyphens"
        )));
    }
    if let Some(expected) = expected_name
        && name != expected
    {
        return Err(invalid(
            "frontmatter `name` must equal the final skill-path segment",
        ));
    }

    let description = frontmatter
        .get("description")
        .ok_or_else(|| invalid("frontmatter is missing the required `description` field"))?
        .as_str()
        .ok_or_else(|| invalid("frontmatter `description` must be a string"))?;
    if description.is_empty() {
        return Err(invalid("frontmatter `description` must not be empty"));
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(invalid(format!(
            "frontmatter `description` exceeds {MAX_DESCRIPTION_CHARS} characters"
        )));
    }

    if let Some(compatibility) = frontmatter.get("compatibility") {
        let compatibility = compatibility
            .as_str()
            .ok_or_else(|| invalid("frontmatter `compatibility` must be a string"))?;
        if compatibility.chars().count() > MAX_COMPATIBILITY_CHARS {
            return Err(invalid(format!(
                "frontmatter `compatibility` exceeds {MAX_COMPATIBILITY_CHARS} characters"
            )));
        }
    }

    if let Some(license) = frontmatter.get("license")
        && !license.is_string()
    {
        return Err(invalid("frontmatter `license` must be a string"));
    }

    if let Some(allowed_tools) = frontmatter.get("allowed-tools") {
        // Agent Skills defines a space-separated string. Claude-compatible
        // team repositories also commonly encode the same bounded vocabulary
        // as a YAML string list; retain those exact source bytes instead of
        // rewriting the Skill during durable import.
        let compatible_list = allowed_tools.as_array().is_some_and(|tools| {
            !tools.is_empty()
                && tools.len() <= 64
                && tools.iter().all(|tool| {
                    tool.as_str()
                        .is_some_and(|tool| !tool.is_empty() && tool.len() <= 128)
                })
        });
        if !allowed_tools.is_string() && !compatible_list {
            return Err(invalid(
                "frontmatter `allowed-tools` must be a space-separated string or bounded string list",
            ));
        }
    }

    if let Some(metadata) = frontmatter.get("metadata") {
        let metadata = metadata
            .as_object()
            .ok_or_else(|| invalid("frontmatter `metadata` must be an object"))?;
        for value in metadata.values() {
            if !value.is_string() {
                return Err(invalid("frontmatter metadata values must be strings"));
            }
        }
    }

    Ok(())
}

/// Extract and parse the YAML frontmatter block from `SKILL.md` content into a
/// JSON object.
///
/// The block must open with `---` on the first line and close with a `---`
/// line. The raw block is size-capped *before* being handed to the YAML parser
/// so an expansion bomb is rejected on bytes rather than after the parser has
/// already grown it in memory.
pub fn parse_skill_md_frontmatter(content: &str) -> Result<Map<String, Value>, ToolError> {
    let without_bom = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = without_bom
        .strip_prefix("---\n")
        .or_else(|| without_bom.strip_prefix("---\r\n"))
        .ok_or_else(|| invalid("SKILL.md must open with a `---` frontmatter delimiter"))?;

    let mut raw = Vec::new();
    let mut budget = 0usize;
    let mut closed = false;
    for line in rest.lines() {
        let line = line.trim_end_matches('\r');
        if line == "---" {
            closed = true;
            break;
        }
        // Running total rather than re-summing the accumulated lines: this loop
        // exists to bound a hostile input, so it must not itself be quadratic in
        // the number of lines that input supplies.
        budget += line.len() + 1;
        if budget > MAX_SKILL_MD_FRONTMATTER_BYTES {
            return Err(invalid(format!(
                "SKILL.md frontmatter exceeds {MAX_SKILL_MD_FRONTMATTER_BYTES} bytes"
            )));
        }
        raw.push(line);
    }
    if !closed {
        return Err(invalid(
            "SKILL.md frontmatter opens with `---` but is never closed",
        ));
    }

    let block = raw.join("\n");
    let parsed: Value = serde_yaml::from_str(&block)
        .map_err(|err| invalid(format!("SKILL.md frontmatter is not valid YAML: {err}")))?;
    match parsed {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(Map::new()),
        _ => Err(invalid("SKILL.md frontmatter must be a YAML mapping")),
    }
}

/// Compare an entry's `frontmatter` against the frontmatter actually present in
/// the fetched `SKILL.md`, field by field.
///
/// SEP-2640 requires this after fetching a `SKILL.md` for which a host holds an
/// entry, and requires any discrepancy to be treated as a verification failure
/// equivalent to a digest mismatch. The comparison is exact and total: a key
/// present on one side and absent on the other is a discrepancy, as is any
/// differing value.
///
/// One representational caveat: YAML admits non-finite floats (`.nan`, `.inf`)
/// and JSON does not, so parsing collapses them to `null`. A `SKILL.md` field
/// written as `.nan` therefore compares equal to a literal `null` in the entry.
/// This cannot mask a meaningful change — `entry.frontmatter` arrives as JSON,
/// which has no way to spell a non-finite float in the first place — but the
/// comparison is exact over JSON values, not over the YAML that produced them.
pub fn compare_frontmatter(
    entry: &Map<String, Value>,
    skill_md: &Map<String, Value>,
) -> Result<(), ToolError> {
    let mut differences = Vec::new();
    for (key, expected) in entry {
        match skill_md.get(key) {
            None => differences.push(format!("`{key}` is missing from SKILL.md")),
            Some(actual) if actual != expected => {
                differences.push(format!("`{key}` differs between the entry and SKILL.md"));
            }
            Some(_) => {}
        }
    }
    for key in skill_md.keys() {
        if !entry.contains_key(key) {
            differences.push(format!(
                "`{key}` is present in SKILL.md but not in the entry"
            ));
        }
    }
    if differences.is_empty() {
        return Ok(());
    }
    differences.sort();
    Err(invalid(format!(
        "SKILL.md frontmatter does not match the skill entry: {}",
        differences.join("; ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    fn minimal() -> Map<String, Value> {
        object(
            json!({ "name": "git-workflow", "description": "Follow the team's Git conventions." }),
        )
    }

    #[test]
    fn accepts_minimal_valid_frontmatter() {
        validate_frontmatter(&minimal(), Some("git-workflow")).expect("valid");
    }

    #[test]
    fn accepts_and_preserves_unknown_keys() {
        let fm = object(json!({
            "name": "git-workflow",
            "description": "d",
            "some-future-field": { "nested": true },
            "metadata": { "io.modelcontextprotocol/unknown": "ignored", "version": "2.1.0" },
        }));
        validate_frontmatter(&fm, Some("git-workflow")).expect("unknown keys pass through");
    }

    #[test]
    fn requires_name_and_description() {
        assert!(validate_frontmatter(&object(json!({ "description": "d" })), None).is_err());
        assert!(validate_frontmatter(&object(json!({ "name": "x" })), None).is_err());
        assert!(
            validate_frontmatter(&object(json!({ "name": "x", "description": "" })), None).is_err()
        );
    }

    #[test]
    fn enforces_name_rules_including_consecutive_hyphens() {
        assert!(is_valid_skill_name("git-workflow"));
        assert!(is_valid_skill_name("a1"));
        assert!(!is_valid_skill_name(""));
        assert!(!is_valid_skill_name("-lead"));
        assert!(!is_valid_skill_name("trail-"));
        assert!(!is_valid_skill_name("double--hyphen"));
        assert!(!is_valid_skill_name("UpperCase"));
        assert!(!is_valid_skill_name("under_score"));
        assert!(!is_valid_skill_name(&"a".repeat(MAX_NAME_CHARS + 1)));
    }

    #[test]
    fn enforces_name_matches_final_path_segment() {
        let err = validate_frontmatter(&minimal(), Some("refunds")).expect_err("mismatch");
        assert!(err.to_string().contains("final skill-path segment"));
    }

    #[test]
    fn enforces_length_caps() {
        let long_description = "d".repeat(MAX_DESCRIPTION_CHARS + 1);
        let fm = object(json!({ "name": "x", "description": long_description }));
        assert!(validate_frontmatter(&fm, None).is_err());

        let long_compatibility = "c".repeat(MAX_COMPATIBILITY_CHARS + 1);
        let fm = object(json!({
            "name": "x", "description": "d", "compatibility": long_compatibility,
        }));
        assert!(validate_frontmatter(&fm, None).is_err());
    }

    #[test]
    fn rejects_wrongly_typed_optional_fields() {
        for fm in [
            json!({ "name": "x", "description": "d", "license": 5 }),
            json!({ "name": "x", "description": "d", "allowed-tools": ["a", 2] }),
            json!({ "name": "x", "description": "d", "metadata": "flat" }),
            json!({ "name": "x", "description": "d", "metadata": { "k": 1 } }),
        ] {
            assert!(validate_frontmatter(&object(fm), None).is_err());
        }
    }

    #[test]
    fn accepts_bounded_claude_compatible_allowed_tools_lists() {
        let fm = object(json!({
            "name": "x",
            "description": "d",
            "allowed-tools": ["Bash", "Read", "Grep"]
        }));
        validate_frontmatter(&fm, None).expect("bounded string list is compatible");
    }

    #[test]
    fn rejects_oversized_frontmatter() {
        let fm = object(json!({
            "name": "x",
            "description": "d",
            "bulk": "z".repeat(MAX_FRONTMATTER_BYTES),
        }));
        assert!(validate_frontmatter(&fm, None).is_err());
    }

    #[test]
    fn accepts_angle_brackets_without_interpreting_them() {
        // The Agent Skills spec warns authors off `<`/`>`; a host must still
        // accept them and simply never interpolate the value anywhere.
        let fm = object(json!({
            "name": "x",
            "description": "Handles <thing> and </other> markers.",
        }));
        validate_frontmatter(&fm, Some("x")).expect("accepted verbatim");
    }

    #[test]
    fn parses_skill_md_frontmatter() {
        let content = "---\nname: git-workflow\ndescription: Follow conventions\n---\n\n# Body\n";
        let parsed = parse_skill_md_frontmatter(content).expect("parsed");
        assert_eq!(parsed.get("name"), Some(&json!("git-workflow")));
        assert_eq!(
            parsed.get("description"),
            Some(&json!("Follow conventions"))
        );
    }

    #[test]
    fn rejects_unopened_or_unclosed_frontmatter() {
        assert!(parse_skill_md_frontmatter("# No frontmatter\n").is_err());
        assert!(parse_skill_md_frontmatter("---\nname: x\nnever closed\n").is_err());
    }

    #[test]
    fn compare_accepts_identical_frontmatter() {
        compare_frontmatter(&minimal(), &minimal()).expect("identical");
    }

    #[test]
    fn compare_rejects_changed_added_and_removed_fields() {
        let entry = minimal();

        let mut changed = minimal();
        changed.insert("description".into(), json!("something else"));
        assert!(compare_frontmatter(&entry, &changed).is_err());

        let mut added = minimal();
        added.insert("allowed-tools".into(), json!("shell"));
        let err = compare_frontmatter(&entry, &added).expect_err("added field is a discrepancy");
        assert!(err.to_string().contains("not in the entry"));

        let mut removed = minimal();
        removed.remove("description");
        assert!(compare_frontmatter(&entry, &removed).is_err());
    }

    #[test]
    fn compare_detects_privilege_widening_injection() {
        // The attack the SEP's comparison requirement exists to stop: an entry
        // a user approved, and a SKILL.md that quietly grants itself more.
        let entry = minimal();
        let mut hostile = minimal();
        hostile.insert("allowed-tools".into(), json!("shell filesystem"));
        assert!(compare_frontmatter(&entry, &hostile).is_err());
    }
}
