//! Operator-provided skills loaded from `$LABBY_HOME/skills`.
//!
//! Embedded first-party skills ship with the binary; these are dropped in by an
//! operator. Both are served under the reserved `labby` origin, so from a
//! client's perspective there is one first-party namespace.
//!
//! # Snapshot at startup, not per request
//!
//! The tree is read once and its digests computed from the bytes read in that
//! same pass. A skill's manifest is what a user's approval binds to, so the
//! digest and the content a client later fetches must come from one read —
//! re-reading per request would let a file change between publishing a digest
//! and serving the file it describes, which is exactly the mismatch a
//! conforming client is required to refuse.
//!
//! The cost is that adding a skill needs a restart. That is the honest trade:
//! live reload would reintroduce the window this avoids.
//!
//! # What is refused
//!
//! Symlinks anywhere in a skill (a link could point outside the root and be
//! served as first-party content), traversal in any relative path, files over
//! the per-file cap, skills over the manifest cap, and any skill whose
//! directory name does not match its frontmatter `name`. A rejected skill is
//! skipped with a logged reason; it never degrades the rest of the tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use labby_runtime::skills::{
    FIRST_PARTY_ORIGIN, ResourceDigest, limits, parse_skill_md_frontmatter,
};

/// Largest single operator-provided file that will be served.
const MAX_LOCAL_SKILL_FILE_BYTES: u64 = 1024 * 1024;

/// Directory under `$LABBY_HOME` scanned for operator skills.
const LOCAL_SKILLS_DIR: &str = "skills";

/// One operator-provided skill, read and digested in a single pass.
#[derive(Debug, Clone)]
pub(crate) struct LocalSkill {
    pub(crate) entry: SkillEntry,
    pub(crate) files: BTreeMap<String, String>,
}

fn local_skills_root() -> PathBuf {
    labby_runtime::lab_home().join(LOCAL_SKILLS_DIR)
}

/// Collect every readable file under `dir`, relative to it.
///
/// Refuses symlinks at any depth rather than resolving them: a link inside an
/// operator's skill directory could point anywhere on the host, and the content
/// would then be served as Labby's own first-party skill.
fn collect_files(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{} is a symlink", path.display()));
        }
        if metadata.is_dir() {
            collect_files(&path, root, out)?;
            continue;
        }
        if metadata.len() > MAX_LOCAL_SKILL_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_LOCAL_SKILL_FILE_BYTES} bytes",
                path.display()
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped the skill root", path.display()))?;
        let Some(relative) = relative.to_str() else {
            return Err(format!("{} is not valid UTF-8", path.display()));
        };
        // Reject traversal in the relative path before it becomes a URI segment.
        labby_runtime::path_safety::reject_path_traversal(relative)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        out.push((relative.replace('\\', "/"), path));
    }
    Ok(())
}

/// Read one skill directory into an entry, or explain why it was skipped.
fn load_skill(name: &str, dir: &Path) -> Result<LocalSkill, String> {
    let mut found = Vec::new();
    collect_files(dir, dir, &mut found)?;
    if found.len() > limits::MAX_RESOURCES_PER_SKILL {
        return Err(format!(
            "holds {} files, over the {} cap",
            found.len(),
            limits::MAX_RESOURCES_PER_SKILL
        ));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));

    let skill_md_uri = format!("skill://{FIRST_PARTY_ORIGIN}/{name}/SKILL.md");
    let mut resources = Vec::with_capacity(found.len());
    let mut files = BTreeMap::new();
    let mut skill_md = None;

    for (relative, path) in found {
        let body = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let uri = format!("skill://{FIRST_PARTY_ORIGIN}/{name}/{relative}");
        // Digest computed from the same bytes that will be served.
        resources.push(SkillResource {
            uri: uri.clone(),
            digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
        });
        if relative == "SKILL.md" {
            skill_md = Some(body.clone());
        }
        files.insert(uri, body);
    }

    let Some(skill_md) = skill_md else {
        return Err("has no SKILL.md".to_string());
    };
    let frontmatter =
        parse_skill_md_frontmatter(&skill_md).map_err(|error| format!("frontmatter: {error}"))?;

    let entry = SkillEntry {
        uri: skill_md_uri,
        frontmatter,
        resources: Some(resources),
        meta: None,
    };
    // Hold operator skills to exactly the bar an upstream's must clear, which
    // includes the directory-name-equals-frontmatter-name rule.
    labby_runtime::skills::validate_skill_entry(&entry)
        .map_err(|reason| reason.as_str().to_string())?;

    Ok(LocalSkill { entry, files })
}

/// Load every operator skill under `$LABBY_HOME/skills`.
///
/// An absent directory is the normal case and yields nothing. A skill that
/// cannot be loaded is skipped with a warning rather than failing the scan —
/// one bad directory must not cost an operator their other skills.
pub(crate) fn load_local_skills() -> BTreeMap<String, LocalSkill> {
    let root = local_skills_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return BTreeMap::new();
    };

    let mut loaded = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        match load_skill(name, &path) {
            Ok(skill) => {
                loaded.insert(name.to_string(), skill);
            }
            Err(reason) => {
                tracing::warn!(
                    skill = %name,
                    reason = %reason,
                    "skipping an operator skill under $LABBY_HOME/skills"
                );
            }
        }
    }
    if !loaded.is_empty() {
        tracing::info!(
            count = loaded.len(),
            root = %root.display(),
            "loaded operator-provided skills"
        );
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, body).expect("write");
    }

    fn valid_skill_md(name: &str) -> String {
        format!("---\nname: {name}\ndescription: an operator skill\n---\n\n# Body\n")
    }

    #[test]
    fn loads_a_well_formed_skill_and_digests_what_it_serves() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("my-skill");
        write(&dir, "SKILL.md", &valid_skill_md("my-skill"));
        write(&dir, "references/notes.md", "notes body");

        let skill = load_skill("my-skill", &dir).expect("loads");
        let resources = skill.entry.resources.as_ref().expect("manifest");
        assert_eq!(resources.len(), 2);
        // Every published digest matches the bytes that will be served — the
        // property a conforming client checks on every read.
        for resource in resources {
            let body = skill.files.get(&resource.uri).expect("served bytes");
            let digest =
                labby_runtime::skills::parse_digest(&resource.digest).expect("valid digest");
            assert!(digest.matches(body.as_bytes()));
        }
        assert!(resources.iter().any(|r| r.uri == skill.entry.uri));
    }

    #[test]
    fn a_symlink_anywhere_in_the_skill_refuses_the_whole_skill() {
        // A link could point outside the root and its target would then be
        // served as Labby's own first-party content.
        #[cfg(unix)]
        {
            let temp = tempfile::tempdir().expect("tempdir");
            let dir = temp.path().join("linky");
            write(&dir, "SKILL.md", &valid_skill_md("linky"));
            let outside = temp.path().join("secret.txt");
            fs::write(&outside, "secret").expect("write");
            std::os::unix::fs::symlink(&outside, dir.join("leak.md")).expect("symlink");

            let error = load_skill("linky", &dir).expect_err("refused");
            assert!(error.contains("symlink"));
        }
    }

    #[test]
    fn a_directory_name_that_disagrees_with_frontmatter_is_refused() {
        // The SEP makes the name recoverable from the URI; a mismatch would
        // make that recovery a lie.
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("stated-name");
        write(&dir, "SKILL.md", &valid_skill_md("different-name"));
        assert!(load_skill("stated-name", &dir).is_err());
    }

    #[test]
    fn a_skill_without_skill_md_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("empty");
        write(&dir, "README.md", "not a skill");
        let error = load_skill("empty", &dir).expect_err("refused");
        assert!(error.contains("SKILL.md"));
    }

    #[test]
    fn an_oversized_file_refuses_the_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("big");
        write(&dir, "SKILL.md", &valid_skill_md("big"));
        write(
            &dir,
            "huge.md",
            &"x".repeat((MAX_LOCAL_SKILL_FILE_BYTES + 1) as usize),
        );
        let error = load_skill("big", &dir).expect_err("refused");
        assert!(error.contains("exceeds"));
    }

    #[test]
    fn too_many_files_refuses_the_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("many");
        write(&dir, "SKILL.md", &valid_skill_md("many"));
        for index in 0..=limits::MAX_RESOURCES_PER_SKILL {
            write(&dir, &format!("f{index}.md"), "x");
        }
        let error = load_skill("many", &dir).expect_err("refused");
        assert!(error.contains("cap"));
    }

    #[test]
    fn an_absent_root_is_the_normal_case_and_yields_nothing() {
        // Most deployments have no operator skills; that must not warn or fail.
        let missing = Path::new("/nonexistent-labby-skills-root");
        let mut out = Vec::new();
        assert!(collect_files(missing, missing, &mut out).is_err());
        assert!(out.is_empty());
    }
}
