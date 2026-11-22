//! Synchronously drain kernel changes before trusting retained quota totals.
use super::{Ledger, adjust_usage};
use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags as Mask, InitFlags, Inotify, WatchDescriptor};
use nix::sys::statfs::{BTRFS_SUPER_MAGIC, EXT4_SUPER_MAGIC, TMPFS_MAGIC, XFS_SUPER_MAGIC, statfs};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct StoreWatch {
    fd: Inotify,
    root_id: (u64, u64),
    paths: HashMap<WatchDescriptor, PathBuf>,
    path_bytes: usize,
}

impl StoreWatch {
    pub(super) fn new(root: &Path) -> Option<Self> {
        let fs = statfs(root).ok()?.filesystem_type();
        if ![
            EXT4_SUPER_MAGIC,
            BTRFS_SUPER_MAGIC,
            TMPFS_MAGIC,
            XFS_SUPER_MAGIC,
        ]
        .contains(&fs)
        {
            return None;
        }
        let meta = root.symlink_metadata().ok()?;
        Some(Self {
            fd: Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC).ok()?,
            root_id: (meta.dev(), meta.ino()),
            paths: HashMap::new(),
            path_bytes: 0,
        })
    }

    fn register(&mut self, path: &Path, meta: &std::fs::Metadata) -> Option<bool> {
        if meta.dev() != self.root_id.0 {
            return None;
        }
        let mask = if meta.is_dir() {
            Mask::IN_ONLYDIR | Mask::IN_CREATE | Mask::IN_DELETE | Mask::IN_MOVE
        } else {
            // Inode watches also see resizes via open handles and other hard links.
            Mask::IN_MODIFY | Mask::IN_ATTRIB
        };
        let wd = self
            .fd
            .add_watch(
                path,
                mask | Mask::IN_MOVE_SELF | Mask::IN_DELETE_SELF | Mask::IN_DONT_FOLLOW,
            )
            .ok()?;
        if let Some(existing) = self.paths.get(&wd) {
            return (existing == path).then_some(false);
        }
        self.path_bytes += path.as_os_str().len() + 96;
        if self.path_bytes > 16 * 1024 * 1024 {
            return None;
        }
        self.paths.insert(wd, path.to_owned());
        Some(true)
    }

    pub(super) fn observe(slot: &mut Option<Self>, path: &Path, meta: &std::fs::Metadata) {
        if slot
            .as_mut()
            .is_some_and(|w| w.register(path, meta).is_none())
        {
            *slot = None;
        }
    }

    /// Lost events or a renamed directory require a new inventory. File events
    /// are reconciled against their current names, regardless of event order.
    fn changed_paths(&mut self, root: &Path) -> Option<HashSet<PathBuf>> {
        let meta = root.symlink_metadata().ok()?;
        if self.root_id != (meta.dev(), meta.ino()) {
            return None;
        }
        let mut changes = HashSet::new();
        for _ in 0..256 {
            let events = match self.fd.read_events() {
                Ok(events) => events,
                Err(Errno::EAGAIN) => return Some(changes),
                Err(_) => return None,
            };
            for e in events {
                if e.mask.intersects(Mask::IN_Q_OVERFLOW | Mask::IN_UNMOUNT) {
                    return None;
                }
                let parent = self.paths.get(&e.wd)?;
                if e.mask.contains(Mask::IN_IGNORED) {
                    self.path_bytes -= parent.as_os_str().len() + 96;
                    self.paths.remove(&e.wd);
                    continue;
                }
                if e.mask.contains(Mask::IN_ISDIR)
                    && e.mask.intersects(Mask::IN_DELETE | Mask::IN_MOVE)
                {
                    return None;
                }
                let path = e
                    .name
                    .as_ref()
                    .map_or_else(|| parent.clone(), |name| parent.join(name));
                if let Ok(rel) = path.strip_prefix(root)
                    && rel
                        .components()
                        .nth(1)
                        .is_some_and(|c| c.as_os_str() == crate::artifact_access::METADATA_DIR)
                    && rel
                        .iter()
                        .next()
                        .and_then(|c| c.to_str())
                        .is_some_and(|s| ulid::Ulid::from_string(s).is_ok())
                {
                    continue;
                }
                changes.insert(path);
            }
        }
        None
    }

    pub(super) fn refresh(&mut self, ledger: &mut Ledger, root: &Path) -> Option<()> {
        use std::io::ErrorKind;
        let mut stack: Vec<_> = self.changed_paths(root)?.into_iter().collect();
        let mut remaining = 16_384usize;
        while let Some(path) = stack.pop() {
            remaining = remaining.checked_sub(1)?;
            let old = ledger.sizes.get(&path).copied();
            let meta = match ledger.usage.metadata(&path) {
                Ok(m) => Some(m),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(_) => return None,
            };
            let size = match meta {
                Some(m) if m.is_file() => {
                    self.register(&path, &m)?;
                    // Read size after watching, including concurrent external writes.
                    Some(ledger.usage.metadata(&path).ok()?.len())
                }
                Some(m) if m.is_dir() => {
                    if !self.register(&path, &m)? {
                        continue;
                    }
                    let run_dir = path.parent() == Some(root)
                        && path
                            .file_name()?
                            .to_str()
                            .is_some_and(|s| ulid::Ulid::from_string(s).is_ok());
                    ledger.usage.run_dirs += usize::from(run_dir);
                    #[cfg(test)]
                    {
                        ledger.usage.directory_reads += 1;
                    }
                    for entry in path.read_dir().ok()? {
                        let entry = entry.ok()?;
                        if !(run_dir && entry.file_name() == crate::artifact_access::METADATA_DIR) {
                            if stack.len() >= remaining {
                                return None;
                            }
                            stack.push(entry.path());
                        }
                    }
                    None
                }
                _ => None,
            };
            if old != size {
                adjust_usage(&mut ledger.usage, root, true, &path, old, size);
                if let Some(size) = size {
                    ledger.sizes.insert(path, size);
                } else {
                    ledger.sizes.remove(&path);
                }
            }
        }
        ledger.usage.runs.retain(|_, (_, files)| *files != 0);
        Some(())
    }
}

#[cfg(test)]
mod tests;
