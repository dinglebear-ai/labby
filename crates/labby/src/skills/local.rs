//! Operator-provided skills loaded from `$LABBY_HOME/skills`.
//!
//! Embedded first-party skills ship with the binary; these are dropped in by an
//! operator. Both are served under the reserved `labby` origin, so from a
//! client's perspective there is one first-party namespace.
//!
//! # Snapshot per generation, not per request
//!
//! The tree is read once and its digests computed from the bytes read in that
//! same pass. A skill's manifest is what a user's approval binds to, so the
//! digest and the content a client later fetches must come from one read —
//! re-reading per request would let a file change between publishing a digest
//! and serving the file it describes, which is exactly the mismatch a
//! conforming client is required to refuse.
//!
//! A refresh builds another complete snapshot and atomically publishes it. A
//! request never rereads these paths after capturing its generation.
//!
//! # What is refused
//!
//! Symlinks anywhere in a skill (a link could point outside the root and be
//! served as first-party content), traversal in any relative path, files over
//! the per-file cap, skills over the manifest cap, and any skill whose
//! directory name does not match its frontmatter `name`. A rejected skill is
//! skipped with a logged reason; it never degrades the rest of the tree.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use labby_runtime::skills::{
    FIRST_PARTY_ORIGIN, ResourceDigest, limits, parse_skill_md_frontmatter,
};

/// Largest single operator-provided file that will be served.
const MAX_LOCAL_SKILL_FILE_BYTES: u64 = limits::MAX_SKILL_RESOURCE_BYTES as u64;

/// Directory under `$LABBY_HOME` scanned for operator skills.
#[cfg(test)]
const LOCAL_SKILLS_DIR: &str = "skills";

/// One operator-provided skill, read and digested in a single pass.
#[derive(Debug, Clone)]
pub(crate) struct LocalSkill {
    pub(crate) entry: SkillEntry,
    pub(crate) files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalSkillRejection {
    pub(crate) skill: String,
    pub(crate) reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct LocalSkillLoad {
    pub(crate) skills: BTreeMap<String, LocalSkill>,
    pub(crate) rejections: Vec<LocalSkillRejection>,
    pub(crate) counters: LocalLoadCounters,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LocalLoadCounters {
    pub(crate) directories_scanned: usize,
    pub(crate) files_scanned: usize,
    pub(crate) files_read: usize,
    pub(crate) bytes_read: usize,
    pub(crate) scan_nanos: u64,
    pub(crate) read_nanos: u64,
    pub(crate) hash_nanos: u64,
    pub(crate) validate_nanos: u64,
    pub(crate) index_nanos: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalLoadLimits {
    pub(crate) active_skills: usize,
    pub(crate) aggregate_bytes: usize,
    pub(crate) per_skill_bytes: usize,
    pub(crate) total_resources: usize,
    pub(crate) live_candidate_bytes: usize,
    pub(crate) bundled_skills: usize,
    pub(crate) bundled_bytes: usize,
    pub(crate) bundled_resources: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalLoadLimit {
    pub(crate) kind: &'static str,
    pub(crate) limit: usize,
    pub(crate) actual: usize,
    pub(crate) counters: LocalLoadCounters,
}

#[cfg(test)]
fn local_skills_root() -> PathBuf {
    labby_runtime::lab_home().join(LOCAL_SKILLS_DIR)
}

/// Collect every readable file under `dir`, relative to it.
///
/// Refuses symlinks at any depth rather than resolving them: a link inside an
/// operator's skill directory could point anywhere on the host, and the content
/// would then be served as Labby's own first-party skill.
fn collect_files(
    dir: &Path,
    root: &Path,
    out: &mut Vec<(String, PathBuf, usize)>,
    counters: &mut LocalLoadCounters,
) -> Result<(), String> {
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
            collect_files(&path, root, out, counters)?;
            continue;
        }
        counters.files_scanned += 1;
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
        out.push((relative.replace('\\', "/"), path, metadata.len() as usize));
    }
    Ok(())
}

/// Read one skill directory into an entry, or explain why it was skipped.
fn load_skill(
    name: &str,
    dir: &Path,
    counters: &mut LocalLoadCounters,
    admission: Option<(LocalLoadLimits, usize, usize, usize)>,
    after_stat: &impl Fn(&Path),
) -> Result<(LocalSkill, usize, usize), SkillLoadError> {
    let mut found = Vec::new();
    let scan_started = Instant::now();
    collect_files(dir, dir, &mut found, counters).map_err(SkillLoadError::Invalid)?;
    counters.scan_nanos = counters
        .scan_nanos
        .saturating_add(scan_started.elapsed().as_nanos() as u64);
    if found.len() > limits::MAX_RESOURCES_PER_SKILL {
        return Err(SkillLoadError::Invalid(format!(
            "holds {} files, over the {} cap",
            found.len(),
            limits::MAX_RESOURCES_PER_SKILL
        )));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let skill_bytes = found.iter().map(|(_, _, bytes)| bytes).sum::<usize>();
    let resource_count = found.len();
    if let Some((limits, local_skills, local_bytes, local_resources)) = admission {
        for (kind, actual, limit) in [
            (
                "active_skills",
                limits.bundled_skills + local_skills + 1,
                limits.active_skills,
            ),
            (
                "aggregate_bytes",
                limits.bundled_bytes + local_bytes + skill_bytes,
                limits.aggregate_bytes,
            ),
            ("per_skill_bytes", skill_bytes, limits.per_skill_bytes),
            (
                "total_resources",
                limits.bundled_resources + local_resources + resource_count,
                limits.total_resources,
            ),
            (
                "live_candidate_bytes",
                limits.bundled_bytes + local_bytes + skill_bytes,
                limits.live_candidate_bytes,
            ),
        ] {
            if actual > limit {
                return Err(SkillLoadError::Limit(LocalLoadLimit {
                    kind,
                    limit,
                    actual,
                    counters: *counters,
                }));
            }
        }
    }
    after_stat(dir);

    let skill_md_uri = format!("skill://{FIRST_PARTY_ORIGIN}/{name}/SKILL.md");
    let mut resources = Vec::with_capacity(found.len());
    let mut files = BTreeMap::new();
    let mut skill_md = None;
    let mut retained_bytes = 0usize;

    for (relative, path, _) in found {
        let current_metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| SkillLoadError::Invalid(format!("{}: {error}", path.display())))?;
        if current_metadata.file_type().is_symlink() {
            return Err(SkillLoadError::Invalid(format!(
                "{} became a symlink before read",
                path.display()
            )));
        }
        let allowed = admission.map_or(
            MAX_LOCAL_SKILL_FILE_BYTES as usize,
            |(limits, _, local_bytes, _)| {
                let aggregate_remaining = limits
                    .aggregate_bytes
                    .saturating_sub(limits.bundled_bytes + local_bytes + retained_bytes);
                let live_remaining = limits
                    .live_candidate_bytes
                    .saturating_sub(limits.bundled_bytes + local_bytes + retained_bytes);
                let skill_remaining = limits.per_skill_bytes.saturating_sub(retained_bytes);
                (MAX_LOCAL_SKILL_FILE_BYTES as usize)
                    .min(aggregate_remaining)
                    .min(live_remaining)
                    .min(skill_remaining)
            },
        );
        let mut bytes = Vec::with_capacity(allowed.min(64 * 1024));
        let read_started = Instant::now();
        std::fs::File::open(&path)
            .and_then(|file| {
                file.take(allowed.saturating_add(1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| SkillLoadError::Invalid(format!("{}: {error}", path.display())))?;
        counters.files_read += 1;
        counters.bytes_read += bytes.len();
        counters.read_nanos = counters
            .read_nanos
            .saturating_add(read_started.elapsed().as_nanos() as u64);
        if bytes.len() > allowed {
            let (limits, _, local_bytes, _) = admission.expect("bounded read has admission");
            let aggregate_limit = limits
                .aggregate_bytes
                .saturating_sub(limits.bundled_bytes + local_bytes);
            let live_limit = limits
                .live_candidate_bytes
                .saturating_sub(limits.bundled_bytes + local_bytes);
            let (kind, limit) = if limits.per_skill_bytes <= aggregate_limit.min(live_limit) {
                ("per_skill_bytes", limits.per_skill_bytes)
            } else if aggregate_limit <= live_limit {
                ("aggregate_bytes", limits.aggregate_bytes)
            } else {
                ("live_candidate_bytes", limits.live_candidate_bytes)
            };
            return Err(SkillLoadError::Limit(LocalLoadLimit {
                kind,
                limit,
                actual: limit.saturating_add(1),
                counters: *counters,
            }));
        }
        let body = String::from_utf8(bytes).map_err(|_| {
            SkillLoadError::Invalid(format!("{} is not valid UTF-8", path.display()))
        })?;
        retained_bytes += body.len();
        let uri = format!("skill://{FIRST_PARTY_ORIGIN}/{name}/{relative}");
        // Digest computed from the same bytes that will be served.
        let hash_started = Instant::now();
        let digest = ResourceDigest::of_bytes(body.as_bytes()).to_wire();
        counters.hash_nanos = counters
            .hash_nanos
            .saturating_add(hash_started.elapsed().as_nanos() as u64);
        resources.push(SkillResource {
            uri: uri.clone(),
            digest,
            size: body.len() as u64,
        });
        if relative == "SKILL.md" {
            skill_md = Some(body.clone());
        }
        files.insert(uri, body);
    }

    let Some(skill_md) = skill_md else {
        return Err(SkillLoadError::Invalid("has no SKILL.md".to_string()));
    };
    let validate_started = Instant::now();
    let frontmatter = parse_skill_md_frontmatter(&skill_md)
        .map_err(|error| SkillLoadError::Invalid(format!("frontmatter: {error}")))?;

    let entry = SkillEntry {
        uri: skill_md_uri,
        frontmatter,
        resources: Some(resources),
        meta: None,
    };
    // Hold operator skills to exactly the bar an upstream's must clear, which
    // includes the directory-name-equals-frontmatter-name rule.
    labby_runtime::skills::validate_skill_entry(&entry)
        .map_err(|reason| SkillLoadError::Invalid(reason.as_str().to_string()))?;
    counters.validate_nanos = counters
        .validate_nanos
        .saturating_add(validate_started.elapsed().as_nanos() as u64);

    Ok((LocalSkill { entry, files }, skill_bytes, resource_count))
}

#[derive(Debug)]
enum SkillLoadError {
    Invalid(String),
    Limit(LocalLoadLimit),
}

/// Load every operator skill under `$LABBY_HOME/skills`.
///
/// An absent directory is the normal case and yields nothing. A skill that
/// cannot be loaded is skipped with a warning rather than failing the scan —
/// one bad directory must not cost an operator their other skills.
#[cfg(test)]
pub(crate) fn load_local_skills() -> BTreeMap<String, LocalSkill> {
    load_local_skills_bounded(&local_skills_root(), None)
        .expect("unbounded local loader cannot exceed a limit")
        .skills
}

pub(crate) fn load_local_skills_bounded(
    root: &Path,
    limits: Option<LocalLoadLimits>,
) -> Result<LocalSkillLoad, LocalLoadLimit> {
    load_local_skills_bounded_with_hook(root, limits, &|_| {})
}

fn load_local_skills_bounded_with_hook(
    root: &Path,
    limits: Option<LocalLoadLimits>,
    after_stat: &impl Fn(&Path),
) -> Result<LocalSkillLoad, LocalLoadLimit> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LocalSkillLoad::default());
        }
        Err(error) => {
            let reason = format!("cannot scan operator skill root: {error}");
            tracing::warn!(
                root = %root.display(),
                reason = %reason,
                "operator skill root is unavailable"
            );
            return Ok(LocalSkillLoad {
                rejections: vec![LocalSkillRejection {
                    skill: "<root>".to_string(),
                    reason,
                }],
                ..LocalSkillLoad::default()
            });
        }
    };

    let mut loaded = LocalSkillLoad::default();
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => loaded.rejections.push(LocalSkillRejection {
                skill: "<directory-entry>".to_string(),
                reason: format!("cannot inspect operator skill entry: {error}"),
            }),
        }
    }
    paths.sort();
    for path in paths {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                loaded.rejections.push(LocalSkillRejection {
                    skill: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("<invalid-name>")
                        .to_string(),
                    reason: format!("cannot inspect operator skill path: {error}"),
                });
                continue;
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        loaded.counters.directories_scanned += 1;
        let local_bytes = loaded.skills.values().map(LocalSkill::byte_len).sum();
        let local_resources = loaded.skills.values().map(LocalSkill::resource_len).sum();
        let admission =
            limits.map(|limits| (limits, loaded.skills.len(), local_bytes, local_resources));
        match load_skill(name, &path, &mut loaded.counters, admission, after_stat) {
            Ok((skill, skill_bytes, skill_resources)) => {
                let _ = (skill_bytes, skill_resources);
                loaded.skills.insert(name.to_string(), skill);
            }
            Err(SkillLoadError::Limit(mut limit)) => {
                limit.counters = loaded.counters;
                return Err(limit);
            }
            Err(SkillLoadError::Invalid(reason)) => {
                tracing::warn!(
                    skill = %name,
                    reason = %reason,
                    "skipping an operator skill under $LABBY_HOME/skills"
                );
                loaded.rejections.push(LocalSkillRejection {
                    skill: name.to_string(),
                    reason,
                });
            }
        }
    }
    if !loaded.skills.is_empty() {
        tracing::info!(
            count = loaded.skills.len(),
            root = %root.display(),
            "loaded operator-provided skills"
        );
    }
    Ok(loaded)
}

impl LocalSkill {
    fn byte_len(&self) -> usize {
        self.files.values().map(String::len).sum()
    }

    fn resource_len(&self) -> usize {
        self.files.len()
    }
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

    fn load_test_skill(name: &str, dir: &Path) -> Result<LocalSkill, SkillLoadError> {
        load_skill(name, dir, &mut LocalLoadCounters::default(), None, &|_| {})
            .map(|(skill, _, _)| skill)
    }

    #[test]
    fn loads_a_well_formed_skill_and_digests_what_it_serves() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("my-skill");
        write(&dir, "SKILL.md", &valid_skill_md("my-skill"));
        write(&dir, "references/notes.md", "notes body");

        let skill = load_test_skill("my-skill", &dir).expect("loads");
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

            let error = load_test_skill("linky", &dir).expect_err("refused");
            assert!(matches!(error, SkillLoadError::Invalid(reason) if reason.contains("symlink")));
        }
    }

    #[test]
    fn a_directory_name_that_disagrees_with_frontmatter_is_refused() {
        // The SEP makes the name recoverable from the URI; a mismatch would
        // make that recovery a lie.
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("stated-name");
        write(&dir, "SKILL.md", &valid_skill_md("different-name"));
        assert!(load_test_skill("stated-name", &dir).is_err());
    }

    #[test]
    fn a_skill_without_skill_md_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("empty");
        write(&dir, "README.md", "not a skill");
        let error = load_test_skill("empty", &dir).expect_err("refused");
        assert!(matches!(error, SkillLoadError::Invalid(reason) if reason.contains("SKILL.md")));
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
        let error = load_test_skill("big", &dir).expect_err("refused");
        assert!(matches!(error, SkillLoadError::Invalid(reason) if reason.contains("exceeds")));
    }

    #[test]
    fn too_many_files_refuses_the_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("many");
        write(&dir, "SKILL.md", &valid_skill_md("many"));
        for index in 0..=limits::MAX_RESOURCES_PER_SKILL {
            write(&dir, &format!("f{index}.md"), "x");
        }
        let error = load_test_skill("many", &dir).expect_err("refused");
        assert!(matches!(error, SkillLoadError::Invalid(reason) if reason.contains("cap")));
    }

    #[test]
    fn an_absent_root_is_the_normal_case_and_yields_nothing() {
        // Most deployments have no operator skills; that must not warn or fail.
        let missing = Path::new("/nonexistent-labby-skills-root");
        let mut out = Vec::new();
        assert!(
            collect_files(
                missing,
                missing,
                &mut out,
                &mut LocalLoadCounters::default()
            )
            .is_err()
        );
        assert!(out.is_empty());
        let loaded = load_local_skills_bounded(missing, None).expect("missing root is normal");
        assert!(loaded.skills.is_empty());
        assert!(loaded.rejections.is_empty());
    }

    #[test]
    fn an_unscannable_root_is_reported_instead_of_looking_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("not-a-directory");
        fs::write(&root, "not a directory").expect("write root file");
        let loaded = load_local_skills_bounded(&root, None).expect("load result");

        assert!(loaded.skills.is_empty());
        assert_eq!(loaded.rejections.len(), 1);
        assert_eq!(loaded.rejections[0].skill, "<root>");
        assert!(loaded.rejections[0].reason.contains("cannot scan"));
    }

    #[test]
    fn aggregate_cap_plus_one_is_rejected_before_any_file_is_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bounded");
        let manifest = valid_skill_md("bounded");
        write(&dir, "SKILL.md", &manifest);
        let limits = LocalLoadLimits {
            active_skills: 1,
            aggregate_bytes: manifest.len() - 1,
            per_skill_bytes: usize::MAX,
            total_resources: 1,
            live_candidate_bytes: usize::MAX,
            bundled_skills: 0,
            bundled_bytes: 0,
            bundled_resources: 0,
        };
        let rejection = load_local_skills_bounded(temp.path(), Some(limits)).unwrap_err();
        assert_eq!(rejection.kind, "aggregate_bytes");
        assert_eq!(rejection.actual, rejection.limit + 1);
        assert_eq!(rejection.counters.files_scanned, 1);
        assert_eq!(rejection.counters.files_read, 0);
        assert_eq!(rejection.counters.bytes_read, 0);
    }

    #[test]
    fn file_growth_after_stat_is_bounded_and_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("racy");
        let manifest = valid_skill_md("racy");
        write(&dir, "SKILL.md", &manifest);
        let limits = LocalLoadLimits {
            active_skills: 1,
            aggregate_bytes: manifest.len(),
            per_skill_bytes: usize::MAX,
            total_resources: 1,
            live_candidate_bytes: usize::MAX,
            bundled_skills: 0,
            bundled_bytes: 0,
            bundled_resources: 0,
        };
        let replacement = format!("{manifest}x");
        let rejection =
            load_local_skills_bounded_with_hook(temp.path(), Some(limits), &|skill_dir| {
                fs::write(skill_dir.join("SKILL.md"), &replacement).expect("replace")
            })
            .unwrap_err();
        assert_eq!(rejection.kind, "aggregate_bytes");
        assert_eq!(rejection.counters.files_read, 1);
        assert_eq!(rejection.counters.bytes_read, manifest.len() + 1);
    }

    #[test]
    fn real_loader_accepts_caps_and_rejects_cap_plus_one_for_each_budget() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("boundary");
        let manifest = valid_skill_md("boundary");
        write(&dir, "SKILL.md", &manifest);
        let base = LocalLoadLimits {
            active_skills: 1,
            aggregate_bytes: manifest.len(),
            per_skill_bytes: manifest.len(),
            total_resources: 1,
            live_candidate_bytes: manifest.len(),
            bundled_skills: 0,
            bundled_bytes: 0,
            bundled_resources: 0,
        };
        assert!(load_local_skills_bounded(temp.path(), Some(base)).is_ok());
        for (kind, limits) in [
            (
                "active_skills",
                LocalLoadLimits {
                    active_skills: 0,
                    ..base
                },
            ),
            (
                "per_skill_bytes",
                LocalLoadLimits {
                    per_skill_bytes: manifest.len() - 1,
                    ..base
                },
            ),
            (
                "total_resources",
                LocalLoadLimits {
                    total_resources: 0,
                    ..base
                },
            ),
            (
                "aggregate_bytes",
                LocalLoadLimits {
                    aggregate_bytes: manifest.len() - 1,
                    ..base
                },
            ),
            (
                "live_candidate_bytes",
                LocalLoadLimits {
                    live_candidate_bytes: manifest.len() - 1,
                    ..base
                },
            ),
        ] {
            let rejection = load_local_skills_bounded(temp.path(), Some(limits)).unwrap_err();
            assert_eq!(rejection.kind, kind);
        }
    }
}
