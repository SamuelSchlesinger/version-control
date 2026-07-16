//! A working-tree stat cache that lets snapshots skip re-reading and
//! re-hashing files that have not changed.
//!
//! # What it is
//!
//! The index maps each file's path to a fingerprint of its filesystem metadata
//! plus the content id we last computed for it. Before hashing a file, a
//! snapshot stat()s it (cheap, no read) and, if the fingerprint still matches,
//! reuses the stored id instead of reading and hashing the bytes. This makes a
//! snapshot's cost proportional to what changed rather than to the size of the
//! whole tree. It is a pure performance memo, decoupled from commit history: it
//! only ever records "the file currently at PATH with this stat hashes to ID".
//!
//! # Why it is safe
//!
//! The classic hazard (git calls it "racy clean") is a file edited in place
//! whose size and mtime happen to be unchanged — a naive cache would miss the
//! edit. Three things close that hole:
//!
//! 1. **ctime.** The fingerprint includes the inode change time, which the OS
//!    bumps on *any* modification and which cannot be moved backward through the
//!    normal filesystem API (`utimensat` sets only atime/mtime). So a content
//!    change always changes the fingerprint, even if mtime is forged.
//! 2. **A racy-clean guard.** An entry is only trusted if the file's mtime is
//!    strictly older (at nanosecond resolution) than the moment the index was
//!    last written, so a file modified around the time of the save is re-hashed.
//! 3. **Store verification.** A cached id is only trusted if the object is still
//!    present in the store, guarding against a pruned or corrupted store.
//!
//! ctime is a Unix concept. On platforms without it (`stat_key` returns `None`),
//! nothing is cached and every file is always re-hashed — slower, but safe.

use std::collections::BTreeMap;
use std::fs::Metadata;

use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;
use crate::object_store::ObjectStore;

/// The filesystem fingerprint used to decide whether a file is unchanged.
struct StatKey {
    size: u64,
    mtime_s: i64,
    mtime_ns: i64,
    ctime_s: i64,
    ctime_ns: i64,
    ino: u64,
}

#[cfg(unix)]
fn stat_key(meta: &Metadata) -> Option<StatKey> {
    use std::os::unix::fs::MetadataExt;
    Some(StatKey {
        size: meta.size(),
        mtime_s: meta.mtime(),
        mtime_ns: meta.mtime_nsec(),
        ctime_s: meta.ctime(),
        ctime_ns: meta.ctime_nsec(),
        ino: meta.ino(),
    })
}

#[cfg(not(unix))]
fn stat_key(_meta: &Metadata) -> Option<StatKey> {
    // No status-change time available, so we cannot cache safely: always
    // re-hash. Slower but never wrong.
    None
}

/// One remembered file: its stat fingerprint and the id its contents hashed to.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    size: u64,
    mtime_s: i64,
    mtime_ns: i64,
    ctime_s: i64,
    ctime_ns: i64,
    ino: u64,
    id: ObjectId,
}

impl IndexEntry {
    fn matches(&self, key: &StatKey) -> bool {
        self.size == key.size
            && self.mtime_s == key.mtime_s
            && self.mtime_ns == key.mtime_ns
            && self.ctime_s == key.ctime_s
            && self.ctime_ns == key.ctime_ns
            && self.ino == key.ino
    }
}

/// The persisted stat cache. Stored as JSON at `.rev/index`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SnapshotIndex {
    entries: BTreeMap<String, IndexEntry>,
    /// Unix time when this index was last written, split into whole seconds and
    /// the sub-second nanoseconds. Used for the racy-clean guard: a file is only
    /// trusted if its mtime is strictly older than this, compared at nanosecond
    /// resolution. Second-only granularity would treat every file touched in the
    /// same second as the write as racy — which, for fast successive snapshots,
    /// is nearly every file, defeating the cache.
    written_at_s: i64,
    #[serde(default)]
    written_at_ns: i64,
}

impl SnapshotIndex {
    /// Returns a trusted content id for the file at `rel` if — and only if — its
    /// current stat matches the remembered fingerprint, its mtime is strictly
    /// before the moment the index was last written (the racy-clean guard), and
    /// the object is still in the store. Otherwise returns `None`, meaning "read
    /// and hash it".
    pub fn trusted_id<Store: ObjectStore>(
        &self,
        rel: &str,
        meta: &Metadata,
        store: &Store,
    ) -> Option<ObjectId> {
        let key = stat_key(meta)?; // None (e.g. non-Unix) => always re-hash
        let entry = self.entries.get(rel)?;
        // Racy-clean guard at nanosecond resolution: only trust a file whose
        // mtime is strictly before the moment the index was written.
        let settled =
            (key.mtime_s, key.mtime_ns) < (self.written_at_s, self.written_at_ns);
        if entry.matches(&key) && settled && store.has(entry.id).unwrap_or(false) {
            Some(entry.id)
        } else {
            None
        }
    }

    /// Records the fingerprint and id for the file at `rel`. On platforms
    /// without a usable stat key, records nothing (so it is re-hashed next time).
    pub fn record(&mut self, rel: &str, meta: &Metadata, id: ObjectId) {
        if let Some(key) = stat_key(meta) {
            self.entries.insert(
                rel.to_string(),
                IndexEntry {
                    size: key.size,
                    mtime_s: key.mtime_s,
                    mtime_ns: key.mtime_ns,
                    ctime_s: key.ctime_s,
                    ctime_ns: key.ctime_ns,
                    ino: key.ino,
                    id,
                },
            );
        }
    }

    /// Stamp the index with the current time (whole seconds + sub-second nanos).
    /// Call this right before saving so the racy-clean guard works on the next
    /// load.
    pub fn mark_written(&mut self, now_unix_secs: i64, now_subsec_ns: i64) {
        self.written_at_s = now_unix_secs;
        self.written_at_ns = now_subsec_ns;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::directory::{Directory, Ignores};
    use crate::object_store::directory::DirectoryObjectStore;
    use std::fs;
    use std::path::Path;

    fn file_id(dir: &Directory, name: &str) -> ObjectId {
        match dir.root.get(name) {
            Some(crate::directory::DirectoryEntry::File(id)) => *id,
            other => panic!("expected file {name}, got {other:?}"),
        }
    }

    /// Sets a file's mtime to a fixed old value (2001-09-09).
    fn backdate_mtime(path: &Path) {
        let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
        filetime::set_file_mtime(path, old).unwrap();
    }

    /// A basic sanity check: a normal edit (which changes mtime) is detected.
    #[test]
    fn edit_that_changes_mtime_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), b"aaaaa").unwrap();

        let mut store = DirectoryObjectStore::new(tmp.path().join("store")).unwrap();
        let ignores = Ignores::new(vec![]);
        let mut index = SnapshotIndex::default();

        let d1 = Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();
        let id1 = file_id(&d1, "a.txt");
        index.mark_written(2_000_000_000, 0);

        // Same size, different content — mtime advances, so it's detected.
        fs::write(root.join("a.txt"), b"bbbbb").unwrap();
        let d2 = Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();
        assert_ne!(id1, file_id(&d2, "a.txt"), "content change was missed");
    }

    /// The safety-critical proof: an in-place edit of identical size, with mtime
    /// forged *backward* to match the cached fingerprint, is still detected —
    /// because ctime advances on the write and cannot be forged back. Without
    /// the ctime check this edit would be silently missed.
    #[test]
    fn edit_with_forged_mtime_is_still_detected_via_ctime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        fs::create_dir(&root).unwrap();
        let file = root.join("a.txt");
        fs::write(&file, b"aaaaa").unwrap();
        backdate_mtime(&file); // old mtime so the entry is not racy-clean

        let mut store = DirectoryObjectStore::new(tmp.path().join("store")).unwrap();
        let ignores = Ignores::new(vec![]);
        let mut index = SnapshotIndex::default();

        let d1 = Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();
        let id1 = file_id(&d1, "a.txt");
        // Written well after the file's (backdated) mtime, so it is trusted, not
        // treated as racy — isolating ctime as the only thing that can catch the
        // edit below.
        index.mark_written(1_900_000_000, 0);

        // Edit in place (same size), then forge mtime back to the old value so
        // size + mtime + inode all match the cached entry.
        fs::write(&file, b"bbbbb").unwrap();
        backdate_mtime(&file);

        let d2 = Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();
        assert_ne!(
            id1,
            file_id(&d2, "a.txt"),
            "an in-place edit with a forged mtime slipped past the cache — ctime guard failed"
        );
    }

    /// A genuinely unchanged file is trusted (its id is reused) once the index
    /// has been written after the file's mtime.
    #[test]
    fn unchanged_file_is_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        fs::create_dir(&root).unwrap();
        let file = root.join("a.txt");
        fs::write(&file, b"hello").unwrap();
        backdate_mtime(&file);

        let mut store = DirectoryObjectStore::new(tmp.path().join("store")).unwrap();
        let ignores = Ignores::new(vec![]);
        let mut index = SnapshotIndex::default();

        let d1 = Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();
        let id1 = file_id(&d1, "a.txt");
        index.mark_written(1_900_000_000, 0);

        // Unchanged file: trusted_id should return the cached id directly.
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(index.trusted_id("a.txt", &meta, &store), Some(id1));
    }

    /// A file modified within the same second the index was written is treated
    /// as racy and re-verified rather than trusted.
    #[test]
    fn racy_clean_files_are_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        fs::create_dir(&root).unwrap();
        let file = root.join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let mut store = DirectoryObjectStore::new(tmp.path().join("store")).unwrap();
        let ignores = Ignores::new(vec![]);
        let mut index = SnapshotIndex::default();
        Directory::from_working_tree(&root, &ignores, &mut store, &mut index, true).unwrap();

        // Index written no later than the file's mtime => racy => not trusted.
        let meta = fs::metadata(&file).unwrap();
        use std::os::unix::fs::MetadataExt;
        index.mark_written(meta.mtime(), meta.mtime_nsec());
        assert_eq!(index.trusted_id("a.txt", &meta, &store), None);
    }
}
