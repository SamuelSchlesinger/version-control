use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{read_dir, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    object_id::ObjectId,
    object_store::ObjectStore
};

/// A directory tree, with [`ObjectId`]s at the leaves.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize, Default)]
pub struct Directory {
    #[serde(flatten)]
    pub root: BTreeMap<String, DirectoryEntry>,
}

#[derive(Debug)]
pub enum Error<Store: ObjectStore> {
    ObjectMissing(ObjectId),
    Store(Store::Error),
    IO(std::io::Error),
    /// A snapshot contained an entry name that is not a safe path component.
    UnsafeEntryName(String),
    /// The working tree is nested more deeply than [`MAX_DIRECTORY_DEPTH`].
    TooDeeplyNested { depth: usize, limit: usize },
}

/// Maximum directory nesting depth a snapshot may contain.
///
/// The tree is stored as nested JSON, and serde's default recursion limit (128)
/// is exceeded at roughly 63 levels — after which the stored snapshot can no
/// longer be *read back*, silently bricking the repository. We refuse to build
/// (and therefore snapshot) a tree deeper than this well-below-the-limit bound,
/// so a bricking snapshot can never be created. 50 levels is far beyond any real
/// source tree.
pub const MAX_DIRECTORY_DEPTH: usize = 50;

/// Checks that a snapshot entry name is a single, ordinary path component.
///
/// Entry names come out of deserialised snapshot JSON, which may have been
/// fetched from an untrusted remote. [`Directory::write`] joins them onto the
/// working-tree path, so a name like `../../.ssh/authorized_keys` would let a
/// remote write anywhere the user can write. Nothing else validates these
/// names, so this is the boundary.
fn is_safe_entry_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return false;
    }
    // Must be exactly one normal component and nothing else.
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(c)), None) if c == name
    )
}

impl<Store: ObjectStore> From<std::io::Error> for Error<Store> {
    fn from(error: std::io::Error) -> Self {
        Error::IO(error)
    }
}

impl<Store: ObjectStore> fmt::Display for Error<Store>
where
    Store::Error: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ObjectMissing(id) => write!(f, "object {id} is missing from the store"),
            Error::Store(e) => write!(f, "{e}"),
            Error::IO(e) => write!(f, "{e}"),
            Error::UnsafeEntryName(name) => {
                write!(f, "snapshot contains an unsafe entry name: {name:?}")
            }
            Error::TooDeeplyNested { depth, limit } => write!(
                f,
                "directory nesting is too deep ({depth} levels); the maximum is {limit}"
            ),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Diff {
    pub deleted: BTreeSet<String>,
    pub added: BTreeMap<String, DirectoryEntry>,
    pub modified: BTreeMap<String, DiffEntry>,
}

impl Diff {
    /// Returns whether this diff contains any content-level diffs
    pub fn has_content_diffs(&self) -> bool {
        for entry in self.modified.values() {
            match entry {
                DiffEntry::FileWithContentDiff { content_diff, .. } => {
                    if content_diff.is_some() {
                        return true;
                    }
                }
                DiffEntry::Directory(diff)
                    if diff.has_content_diffs() => {
                        return true;
                    }
                _ => {}
            }
        }
        false
    }

    /// This approach doesn't work well for production because we can't mutate the nested Directory
    /// objects without cloning them. Instead, generate content diffs at creation time in
    /// the Directory::diff_with_content method
    #[deprecated]
    pub fn with_content_diffs<Store: ObjectStore>(&mut self, _store: &Store) -> &mut Self {
        // This approach won't work properly - directory diffs need to be created with content
        // at generation time
        self
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A file was modified, contains new ObjectId
    File(ObjectId),
    /// A file was modified with content diff available
    FileWithContentDiff {
        /// New object ID
        new_id: ObjectId,
        /// Optional content diff
        #[serde(skip)]
        content_diff: Option<crate::content_diff::ContentDiff>,
    },
    /// A directory was modified
    Directory(Box<Diff>),
}

impl DirectoryEntry {
    pub fn diff(&self, other: &DirectoryEntry) -> Option<DiffEntry> {
        use DirectoryEntry::*;
        match (self, other) {
            (File(id), File(id_)) => {
                if id != id_ {
                    // Files are different, create a basic file diff entry
                    Some(DiffEntry::FileWithContentDiff {
                        new_id: *id_,
                        content_diff: None, // Content diff will be populated later when requested
                    })
                } else {
                    None
                }
            }
            (Directory(_), File(id)) => Some(DiffEntry::File(*id)),
            (File(_), Directory(d)) => Some(DiffEntry::Directory(Box::new(Diff {
                deleted: BTreeSet::new(),
                added: d.root.clone(),
                modified: BTreeMap::new(),
            }))),
            (Directory(d), Directory(d_)) => {
                if d == d_ {
                    None
                } else {
                    Some(DiffEntry::Directory(Box::new(d.diff(d_))))
                }
            }
        }
    }

    // Generate content diff for a file entry
    pub fn generate_content_diff<Store: ObjectStore>(
        &self,
        other: &DirectoryEntry,
        store: &Store
    ) -> Option<DiffEntry> {
        use DirectoryEntry::*;
        match (self, other) {
            (File(old_id), File(new_id)) => {
                if old_id != new_id {
                    match crate::content_diff::ContentDiff::generate(store, *old_id, *new_id) {
                        Ok(content_diff) => Some(DiffEntry::FileWithContentDiff {
                            new_id: *new_id,
                            content_diff,
                        }),
                        Err(_) => Some(DiffEntry::File(*new_id)),
                    }
                } else {
                    None
                }
            }
            // For other cases, fall back to the regular diff
            _ => self.diff(other),
        }
    }
}

impl Directory {
    /// Iterate over all files in the directory tree, returning (path, ObjectId) pairs
    pub fn files(&self) -> Vec<(PathBuf, ObjectId)> {
        let mut files = Vec::new();
        self.collect_files(&mut files, PathBuf::new());
        files
    }
    
    fn collect_files(&self, files: &mut Vec<(PathBuf, ObjectId)>, current_path: PathBuf) {
        for (name, entry) in &self.root {
            let path = current_path.join(name);
            match entry {
                DirectoryEntry::File(id) => {
                    files.push((path, *id));
                },
                DirectoryEntry::Directory(dir) => {
                    dir.collect_files(files, path);
                }
            }
        }
    }

    /// The deepest directory nesting in this tree (0 for a flat tree of files).
    pub fn max_depth(&self) -> usize {
        self.root
            .values()
            .map(|entry| match entry {
                DirectoryEntry::Directory(dir) => 1 + dir.max_depth(),
                DirectoryEntry::File(_) => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// Rejects a tree nested more deeply than [`MAX_DIRECTORY_DEPTH`], before it
    /// can be stored and later become unreadable (see the constant's docs).
    fn reject_if_too_deep<Store: ObjectStore>(&self) -> Result<(), Error<Store>> {
        let depth = self.max_depth();
        if depth > MAX_DIRECTORY_DEPTH {
            return Err(Error::TooDeeplyNested {
                depth,
                limit: MAX_DIRECTORY_DEPTH,
            });
        }
        Ok(())
    }
    
    /// Compute the diff between this directory structure and the one
    /// which is currently located at the path.
    pub fn diff(&self, other: &Directory) -> Diff {
        // Use InMemoryObjectStore as a type parameter but pass None as the store
        // since we're not using content diffs
        self.diff_internal::<crate::object_store::in_memory::InMemoryObjectStore>(other, false, None)
    }

    /// Compute a diff with optional content diffing between file contents.
    ///
    /// When `with_content_diff` is true and an object store is provided,
    /// the diff will include line-by-line differences between modified files.
    pub fn diff_with_content<Store: ObjectStore>(
        &self,
        other: &Directory,
        with_content_diff: bool,
        store: &Store
    ) -> Diff {
        self.diff_internal(other, with_content_diff, Some(store))
    }

    // Internal implementation that handles both regular and content diffing
    fn diff_internal<Store: ObjectStore>(
        &self,
        other: &Directory,
        with_content_diff: bool,
        store: Option<&Store>
    ) -> Diff {
        let added: BTreeMap<String, DirectoryEntry> = other
            .root
            .iter()
            .filter(|(file_name, _dir_entry)| !self.root.contains_key(*file_name))
            .map(|(fname, dir_entry)| (fname.clone(), dir_entry.clone()))
            .collect();

        let deleted: BTreeSet<String> = self
            .root
            .iter()
            .filter(|(file_name, _dir_entry)| !other.root.contains_key(*file_name))
            .map(|(fname, _dir_entry)| fname.clone())
            .collect();

        let modified: BTreeMap<String, DiffEntry> = self
            .root
            .iter()
            .filter_map(|(file_name, dir_entry)| {
                other.root.get(file_name).and_then(|other_dir_entry| {
                    // If content diffing is enabled and we have an object store
                    if let (true, Some(store)) = (with_content_diff, store) {
                        match (dir_entry, other_dir_entry) {
                            // For files, use the content diff method
                            (DirectoryEntry::File(_), DirectoryEntry::File(_)) => {
                                dir_entry
                                    .generate_content_diff(other_dir_entry, store)
                                    .map(|diff| (file_name.clone(), diff))
                            }
                            // For directories, recursively apply content diffing —
                            // but only if they actually differ. Without this
                            // equality check (which the non-content path performs
                            // via DirectoryEntry::diff) every unchanged directory
                            // was reported as modified with an empty diff body.
                            (DirectoryEntry::Directory(dir), DirectoryEntry::Directory(other_dir)) => {
                                if dir == other_dir {
                                    None
                                } else {
                                    let nested_diff =
                                        dir.diff_with_content(other_dir, with_content_diff, store);
                                    Some((file_name.clone(), DiffEntry::Directory(Box::new(nested_diff))))
                                }
                            }
                            // For other mixed types, use regular diff
                            _ => dir_entry
                                .diff(other_dir_entry)
                                .map(|diff| (file_name.clone(), diff))
                        }
                    } else {
                        // If content diffing is disabled, use the regular diff
                        dir_entry
                            .diff(other_dir_entry)
                            .map(|diff| (file_name.clone(), diff))
                    }
                })
            })
            .collect();

        Diff {
            added,
            deleted,
            modified,
        }
    }

    /// Write out the directory structure at the given directory path.
    ///
    /// The target directory must already exist.
    /// If delete_absent is true, files in the target directory that are not in
    /// this Directory will be deleted.
    pub fn write<Store: ObjectStore>(
        &self,
        store: &Store,
        path: &Path,
        delete_absent: bool,
    ) -> Result<(), Error<Store>> {
        // Previously this was `if read_dir(path).is_ok()`, which turned an
        // unreadable or missing target directory into a silent success.
        read_dir(path)?;

        // Reject the whole write before touching the disk if any entry name is
        // unsafe, so a malicious snapshot cannot half-apply.
        self.check_entry_names_recursively()?;

        // First, write/update all files and directories in the snapshot
        for (file_name, entry) in self.root.iter() {
            match entry {
                DirectoryEntry::File(id) => {
                    let v = store.read(*id).map_err(Error::Store)?;
                    match v {
                        Some(v) => {
                            let target = path.join(file_name);
                            // Never write *through* an existing symlink (which
                            // could point outside the repo) or into a directory
                            // sitting where a file belongs. Replace whatever is
                            // there with a fresh regular file, exactly as git
                            // does on checkout.
                            remove_non_regular_file(&target)?;
                            let mut f = File::options()
                                .create(true)
                                .write(true)
                                .truncate(true)
                                .open(&target)?;
                            f.write_all(&v)?;
                        }
                        None => return Err(Error::ObjectMissing(*id)),
                    }
                }
                DirectoryEntry::Directory(dir) => {
                    let dir_path = path.join(file_name);

                    // Ensure a *real* directory is here before descending. If a
                    // symlink (or other non-directory) occupies this path, remove
                    // it and create a real directory — otherwise the recursive
                    // write would follow the symlink and escape the repository.
                    ensure_real_directory(&dir_path)?;

                    dir.write(store, dir_path.as_path(), delete_absent)?;
                }
            }
        }

        // If delete_absent is true, remove files that aren't in the snapshot
        if delete_absent {
            for entry in read_dir(path)? {
                let entry = entry?;
                // A non-UTF-8 name cannot be in `self.root` (whose keys are
                // Strings), so it is by definition absent from the snapshot.
                // Previously `.into_string().unwrap()` panicked here instead.
                let file_name = match entry.file_name().into_string() {
                    Ok(name) => name,
                    Err(raw) => {
                        log::warn!(
                            "skipping file with non-UTF-8 name during delete pass: {raw:?}"
                        );
                        continue;
                    }
                };

                // Skip special directories like .rev, .git, etc.
                if file_name == ".rev" || file_name == ".git" {
                    continue;
                }

                if !self.root.contains_key(&file_name) {
                    let entry_path = entry.path();
                    if entry.file_type()?.is_dir() {
                        std::fs::remove_dir_all(&entry_path)?;
                    } else {
                        std::fs::remove_file(&entry_path)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Validates every entry name in this tree before any of it is written.
    fn check_entry_names_recursively<Store: ObjectStore>(&self) -> Result<(), Error<Store>> {
        for (file_name, entry) in self.root.iter() {
            if !is_safe_entry_name(file_name) {
                return Err(Error::UnsafeEntryName(file_name.clone()));
            }
            if let DirectoryEntry::Directory(dir) = entry {
                dir.check_entry_names_recursively::<Store>()?;
            }
        }
        Ok(())
    }
}

/// Represents patterns to ignore when creating a directory structure.
/// Supports gitignore-style glob patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ignores {
    /// The patterns to ignore
    pub patterns: Vec<String>,
    /// This field is populated at runtime and not serialized
    #[serde(skip)]
    glob_set: Option<GlobSet>,
}

impl PartialEq for Ignores {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for Ignores {}

impl Default for Ignores {
    fn default() -> Self {
        let patterns = vec![
            String::from(".rev"),
            String::from("target"),
            String::from(".git"),
            String::from("**/*.class"),
            String::from("**/*.o"),
            String::from("**/*.so"),
            String::from("**/*.dylib"),
            String::from("**/*.exe"),
        ];

        let mut ignores = Ignores {
            patterns,
            glob_set: None,
        };

        // Pre-build the glob set
        ignores.build_glob_set();

        ignores
    }
}

impl Ignores {
    /// Create a new Ignores from a list of patterns
    pub fn new(patterns: Vec<String>) -> Self {
        let mut ignores = Ignores {
            patterns,
            glob_set: None,
        };

        ignores.build_glob_set();

        ignores
    }

    /// Build the glob set from the patterns
    fn build_glob_set(&mut self) {
        let mut builder = GlobSetBuilder::new();

        for pattern in &self.patterns {
            // A trailing slash is gitignore's "directory only" marker (`build/`).
            // The glob crate would match it literally and never fire against the
            // directory entry `build` or the paths under it, so the pattern
            // silently did nothing. Strip it so `build/` behaves like `build`.
            let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
            if pattern.is_empty() {
                continue;
            }
            match Glob::new(pattern) {
                Ok(glob) => { builder.add(glob); },
                Err(e) => { log::warn!("Invalid glob pattern '{pattern}': {e}"); }
            }
        }

        match builder.build() {
            Ok(glob_set) => { self.glob_set = Some(glob_set); },
            Err(e) => { log::error!("Failed to build glob set: {e}"); }
        }
    }

    /// Check if a path is ignored
    pub fn is_ignored(&self, path: &Path) -> bool {
        // Make sure the glob set is built
        if self.glob_set.is_none() {
            let mut this = self.clone();
            this.build_glob_set();
            return this.is_ignored(path);
        }

        // Match against the glob set
        if let Some(glob_set) = &self.glob_set {
            // Check both the full path and just the file name
            let path_str = path.to_string_lossy();
            if glob_set.is_match(path_str.to_string()) {
                return true;
            }

            // Check just the file name
            if let Some(file_name) = path.file_name() {
                let file_name_str = file_name.to_string_lossy().to_string();
                if glob_set.is_match(&file_name_str) {
                    return true;
                }
            }
        }

        false
    }

    /// Add a pattern to the ignore list
    pub fn add_pattern(&mut self, pattern: String) {
        self.patterns.push(pattern);
        self.build_glob_set();
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum DirectoryEntry {
    Directory(Box<Directory>),
    File(ObjectId),
}

impl Directory {
    pub fn new<Store: ObjectStore>(
        dir: &Path,
        ignores: &Ignores,
        store: &mut Store,
    ) -> Result<Self, Error<Store>> {
        let mut root = BTreeMap::new();
        for f in std::fs::read_dir(dir).map_err(Error::IO)? {
            let dir_entry = f.map_err(Error::IO)?;
            let path = dir_entry.path();

            // Check if the file or directory should be ignored
            if ignores.is_ignored(&path) {
                continue;
            }

            // A non-UTF-8 name can't be a key in our String-keyed tree; skip it
            // with a warning rather than panicking (these are legal on Linux).
            let file_name = match utf8_entry_name(&dir_entry) {
                Some(name) => name,
                None => continue,
            };

            let file_type = dir_entry.file_type().map_err(Error::IO)?;
            if file_type.is_dir() {
                let directory = Directory::new(path.as_path(), ignores, store)?;
                root.insert(file_name, DirectoryEntry::Directory(Box::new(directory)));
            } else if file_type.is_file() {
                // Read and hash the file exactly once: insert() hashes the bytes
                // and returns the id, so a separate ObjectId::try_from (which
                // reopened and re-hashed the file) was pure duplicated work.
                let mut v = Vec::new();
                File::options()
                    .read(true)
                    .open(&path)
                    .map_err(Error::IO)?
                    .read_to_end(&mut v)
                    .map_err(Error::IO)?;
                let id = store.insert(&v).map_err(Error::Store)?;
                root.insert(file_name, DirectoryEntry::File(id));
            } else {
                log::warn!(
                    "Skipping unsupported file type (not a regular file or directory): {:?}",
                    path
                );
            }
        }
        let directory = Directory { root };
        directory.reject_if_too_deep()?;
        Ok(directory)
    }

    /// Builds the tree from the working directory using a [`SnapshotIndex`] so
    /// unchanged files are not re-read or re-hashed.
    ///
    /// `root` is the working-tree root (used to compute the index keys). When
    /// `consult` is false the index is not read from (every file is hashed), but
    /// fresh fingerprints are still recorded — this is the `--rehash` path.
    pub fn from_working_tree<Store: ObjectStore>(
        root: &Path,
        ignores: &Ignores,
        store: &mut Store,
        index: &mut crate::snapshot_index::SnapshotIndex,
        consult: bool,
    ) -> Result<Self, Error<Store>> {
        let directory = Self::build_indexed(root, root, ignores, store, index, consult)?;
        directory.reject_if_too_deep()?;
        Ok(directory)
    }

    fn build_indexed<Store: ObjectStore>(
        root: &Path,
        dir: &Path,
        ignores: &Ignores,
        store: &mut Store,
        index: &mut crate::snapshot_index::SnapshotIndex,
        consult: bool,
    ) -> Result<Self, Error<Store>> {
        let mut map = BTreeMap::new();
        for f in read_dir(dir).map_err(Error::IO)? {
            let dir_entry = f.map_err(Error::IO)?;
            let path = dir_entry.path();

            if ignores.is_ignored(&path) {
                continue;
            }

            let file_name = match utf8_entry_name(&dir_entry) {
                Some(name) => name,
                None => continue,
            };

            let file_type = dir_entry.file_type().map_err(Error::IO)?;
            if file_type.is_dir() {
                let sub = Self::build_indexed(root, path.as_path(), ignores, store, index, consult)?;
                map.insert(file_name, DirectoryEntry::Directory(Box::new(sub)));
            } else if file_type.is_file() {
                let meta = dir_entry.metadata().map_err(Error::IO)?;
                let rel = relative_key(root, &path);

                // Trust the cache only when explicitly consulting it.
                let id = match consult
                    .then(|| index.trusted_id(&rel, &meta, store))
                    .flatten()
                {
                    Some(id) => id,
                    None => {
                        let mut v = Vec::new();
                        File::options()
                            .read(true)
                            .open(&path)
                            .map_err(Error::IO)?
                            .read_to_end(&mut v)
                            .map_err(Error::IO)?;
                        let id = store.insert(&v).map_err(Error::Store)?;
                        index.record(&rel, &meta, id);
                        id
                    }
                };
                map.insert(file_name, DirectoryEntry::File(id));
            } else {
                log::warn!(
                    "Skipping unsupported file type (not a regular file or directory): {:?}",
                    path
                );
            }
        }
        Ok(Directory { root: map })
    }
}

/// Returns a directory entry's file name as a `String`, or `None` (with a
/// warning) if it is not valid UTF-8. Our tree keys are `String`s, so a
/// non-UTF-8 name (legal on Linux) cannot be represented and is skipped rather
/// than panicking the whole command.
fn utf8_entry_name(entry: &std::fs::DirEntry) -> Option<String> {
    match entry.file_name().into_string() {
        Ok(name) => Some(name),
        Err(raw) => {
            log::warn!("skipping file with a non-UTF-8 name: {raw:?}");
            None
        }
    }
}

/// Removes whatever is at `target` unless it is already a regular file.
///
/// This is the write-side defense against symlink attacks: a working-tree
/// symlink at a tracked path must be unlinked (not followed) before we write,
/// so content is never written through it to an arbitrary location. A directory
/// occupying a file's path (a file/directory conflict) is likewise removed. A
/// plain regular file is left in place for the caller to truncate and overwrite.
fn remove_non_regular_file(target: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(meta) => {
            if meta.file_type().is_dir() {
                std::fs::remove_dir_all(target)
            } else {
                // Symlink, fifo, socket, etc. remove_file unlinks the entry
                // itself, never following a symlink to its target.
                std::fs::remove_file(target)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Ensures `dir_path` is a real directory, creating one if nothing is there and
/// replacing any symlink (or other non-directory) that occupies the path. This
/// stops the recursive writer from descending *through* a symlinked directory,
/// which would let a snapshot write outside the repository.
fn ensure_real_directory(dir_path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir_path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => {
            // A symlink or a file is in the way: remove it, then make a real dir.
            std::fs::remove_file(dir_path)?;
            std::fs::create_dir(dir_path)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(dir_path),
        Err(e) => Err(e),
    }
}

/// The index key for a file: its path relative to the working-tree root, with
/// forward slashes so the key is stable across platforms.
fn relative_key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This enum now carries a content_diff option for file changes
        enum DiffStackItem {
            Deleted(PathBuf),
            Added(PathBuf, DirectoryEntry),
            Modified(PathBuf, DiffEntry),
        }
        let mut stack: Vec<DiffStackItem> = vec![];

        for (path, dir_entry) in self.added.clone() {
            stack.push(DiffStackItem::Added(PathBuf::from(path), dir_entry));
        }
        for (path, diff_entry) in self.modified.clone() {
            stack.push(DiffStackItem::Modified(PathBuf::from(path), diff_entry));
        }
        for path in self.deleted.clone() {
            stack.push(DiffStackItem::Deleted(PathBuf::from(path)));
        }

        // This enum now includes a variant for files with content diffs
        enum DiffItem {
            Deleted,
            Added,
            Modified,
            // Special variant for modified files with content diffs
            ModifiedWithContent(crate::content_diff::ContentDiff),
        }
        let mut diff_paths: BTreeMap<PathBuf, DiffItem> = BTreeMap::new();

        while let Some(diff_stack_item) = stack.pop() {
            match diff_stack_item {
                DiffStackItem::Deleted(path) => {
                    diff_paths.insert(path, DiffItem::Deleted);
                }
                DiffStackItem::Added(path, dir_entry) => match dir_entry {
                    DirectoryEntry::File(_) => {
                        diff_paths.insert(path, DiffItem::Added);
                    }
                    DirectoryEntry::Directory(dir) => {
                        if dir.root.is_empty() {
                            diff_paths.insert(path, DiffItem::Added);
                        } else {
                            // Always add the directory itself as added
                            diff_paths.insert(path.clone(), DiffItem::Added);

                            // And also add all of its contents recursively
                            for (dir_name, dir_entry) in dir.root.clone() {
                                stack.push(DiffStackItem::Added(path.join(dir_name), dir_entry));
                            }
                        }
                    }
                },
                DiffStackItem::Modified(path, diff_entry) => match diff_entry {
                    DiffEntry::File(_) => {
                        diff_paths.insert(path, DiffItem::Modified);
                    }
                    DiffEntry::FileWithContentDiff { content_diff, .. } => {
                        if let Some(content_diff) = content_diff {
                            diff_paths.insert(path, DiffItem::ModifiedWithContent(content_diff));
                        } else {
                            diff_paths.insert(path, DiffItem::Modified);
                        }
                    }
                    DiffEntry::Directory(diff) => {
                        // Always add the directory itself as modified
                        diff_paths.insert(path.clone(), DiffItem::Modified);

                        // And recursively process all changes inside it
                        for (dir_name, dir_entry) in diff.added.clone() {
                            stack.push(DiffStackItem::Added(path.join(dir_name), dir_entry))
                        }
                        for (dir_name, diff_entry) in diff.modified.clone() {
                            stack.push(DiffStackItem::Modified(path.join(dir_name), diff_entry))
                        }
                        for dir_name in diff.deleted.clone() {
                            stack.push(DiffStackItem::Deleted(path.join(dir_name)))
                        }
                    }
                },
            }
        }

        for (path, diff_item) in diff_paths {
            match diff_item {
                DiffItem::Deleted => writeln!(f, "D {}", path.to_str().unwrap())?,
                DiffItem::Added => writeln!(f, "A {}", path.to_str().unwrap())?,
                DiffItem::Modified => writeln!(f, "M {}", path.to_str().unwrap())?,
                DiffItem::ModifiedWithContent(content_diff) => {
                    writeln!(f, "M {}", path.to_str().unwrap())?;
                    // Format a delimited content diff section
                    writeln!(f, "<<<<<<< CONTENT DIFF: {} >>>>>>>", path.to_str().unwrap())?;
                    writeln!(f, "{}", content_diff)?;
                    writeln!(f, "<<<<<<< END CONTENT DIFF >>>>>>>>")?;
                }
            }
        }
        Ok(())
    }
}

#[test]
fn test_diff_display() {
    let diff_empty: Diff = Diff {
        deleted: BTreeSet::new(),
        added: BTreeMap::new(),
        modified: BTreeMap::new(),
    };
    assert_eq!(diff_empty.to_string(), "");

    let deleted_foo = BTreeSet::from([String::from("foo")]);
    let added_bar: BTreeMap<String, DirectoryEntry> = vec![(
        String::from("bar"),
        DirectoryEntry::File(ObjectId::from(&vec![])),
    )]
    .into_iter()
    .collect();

    let diff_1: Diff = Diff {
        deleted: BTreeSet::new(),
        added: added_bar.clone(),
        modified: BTreeMap::new(),
    };
    assert_eq!(diff_1.to_string(), "A bar\n");

    let diff_2: Diff = Diff {
        deleted: deleted_foo.clone(),
        added: BTreeMap::new(),
        modified: BTreeMap::new(),
    };
    assert_eq!(diff_2.to_string(), "D foo\n");

    let diff_3: Diff = Diff {
        deleted: deleted_foo.clone(),
        added: added_bar.clone(),
        modified: BTreeMap::new(),
    };
    assert_eq!(diff_3.to_string(), ["A bar", "D foo", ""].join("\n"));

    let diff_4: Diff = Diff {
        deleted: deleted_foo.clone(),
        added: added_bar.clone(),
        modified: vec![(
            String::from("baz"),
            DiffEntry::File(ObjectId::from(&vec![])),
        )]
        .into_iter()
        .collect(),
    };
    assert_eq!(
        diff_4.to_string(),
        ["A bar", "M baz", "D foo", ""].join("\n")
    );

    let diff_5: Diff = Diff {
        deleted: deleted_foo.clone(),
        added: added_bar.clone(),
        modified: vec![
            (String::from("a"), DiffEntry::Directory(Box::new(diff_2))),
            (String::from("baz"), DiffEntry::Directory(Box::new(diff_4))),
        ]
        .into_iter()
        .collect(),
    };
    // Now we also show the directories themselves, not just their contents
    assert_eq!(
        diff_5.to_string(),
        [
            "M a",
            "D a/foo",
            "A bar",
            "M baz",
            "A baz/bar",
            "M baz/baz",
            "D baz/foo",
            "D foo",
            ""
        ]
        .join("\n")
    );
}

#[test]
fn test_directory() {
    use crate::object_store::in_memory::InMemoryObjectStore;
    use std::env::current_dir;
    let dir = current_dir().unwrap();
    let mut store = InMemoryObjectStore::new();

    // Create ignores with the default patterns
    let ignores = Ignores::default();

    let codebase = Directory::new(
        dir.as_path(),
        &ignores,
        &mut store,
    )
    .unwrap();
    let readme_path = String::from("README.md");
    assert!(codebase.root.contains_key(&readme_path));
}

#[test]
fn test_ignores_glob_patterns() {
    use std::path::PathBuf;

    // Create an Ignores with some test patterns
    let ignores = Ignores::new(vec![
        String::from("*.log"),
        String::from("build"),
        String::from("**/*.tmp"),
        String::from("docs/*.md"),
        String::from("[abc].txt"),
    ]);

    // Test direct file matches
    assert!(ignores.is_ignored(&PathBuf::from("file.log")));
    assert!(ignores.is_ignored(&PathBuf::from("error.log")));
    assert!(!ignores.is_ignored(&PathBuf::from("file.txt")));

    // Test directory matches
    assert!(ignores.is_ignored(&PathBuf::from("build")));
    // This won't work because our simple implementation doesn't have full gitignore semantics
    // assert!(ignores.is_ignored(&PathBuf::from("build/output.txt")));
    assert!(!ignores.is_ignored(&PathBuf::from("src/build.rs")));

    // Test recursive glob matches
    assert!(ignores.is_ignored(&PathBuf::from("file.tmp")));
    assert!(ignores.is_ignored(&PathBuf::from("subdir/file.tmp")));
    assert!(ignores.is_ignored(&PathBuf::from("deep/nested/dir/file.tmp")));

    // Test specific directory pattern matches
    assert!(ignores.is_ignored(&PathBuf::from("docs/readme.md")));
    assert!(!ignores.is_ignored(&PathBuf::from("src/docs/readme.md")));

    // Test character class
    assert!(ignores.is_ignored(&PathBuf::from("a.txt")));
    assert!(ignores.is_ignored(&PathBuf::from("b.txt")));
    assert!(!ignores.is_ignored(&PathBuf::from("d.txt")));
}

#[test]
fn test_ignores_add_pattern() {
    let mut ignores = Ignores::default();
    let initial_count = ignores.patterns.len();

    // Add a new pattern
    ignores.add_pattern(String::from("*.docx"));
    assert_eq!(ignores.patterns.len(), initial_count + 1);
    assert!(ignores.is_ignored(&PathBuf::from("document.docx")));

    // Try to match the new pattern
    assert!(ignores.is_ignored(&PathBuf::from("document.docx")));
}

#[test]
fn test_unsafe_entry_names_are_rejected() {
    assert!(is_safe_entry_name("a.txt"));
    assert!(is_safe_entry_name("dir"));
    assert!(is_safe_entry_name("weird name with spaces"));
    assert!(is_safe_entry_name("..dotfile"));

    for bad in ["", ".", "..", "../evil", "a/b", "a\\b", "/abs", "\0nul"] {
        assert!(!is_safe_entry_name(bad), "expected {bad:?} to be rejected");
    }
}

#[test]
fn test_malicious_snapshot_cannot_write_outside_target() {
    use crate::object_store::in_memory::InMemoryObjectStore;

    let tempdir = tempfile::tempdir().unwrap();
    let target = tempdir.path().join("repo");
    std::fs::create_dir(&target).unwrap();

    let mut store = InMemoryObjectStore::default();
    let id = store.insert(b"owned").unwrap();

    // A snapshot as it might arrive from an untrusted remote.
    let mut root = BTreeMap::new();
    root.insert("../escaped.txt".to_string(), DirectoryEntry::File(id));
    let evil = Directory { root };

    let err = evil.write(&store, &target, false);
    assert!(matches!(err, Err(Error::UnsafeEntryName(_))), "got {err:?}");
    assert!(
        !tempdir.path().join("escaped.txt").exists(),
        "write escaped the target directory"
    );

    // And nested one level down.
    let mut inner = BTreeMap::new();
    inner.insert("../../escaped2.txt".to_string(), DirectoryEntry::File(id));
    let mut outer = BTreeMap::new();
    outer.insert(
        "sub".to_string(),
        DirectoryEntry::Directory(Box::new(Directory { root: inner })),
    );
    let evil2 = Directory { root: outer };

    assert!(matches!(
        evil2.write(&store, &target, false),
        Err(Error::UnsafeEntryName(_))
    ));
    assert!(!tempdir.path().join("escaped2.txt").exists());
}

#[cfg(all(test, unix))]
#[test]
fn test_write_never_follows_symlinks() {
    use crate::object_store::in_memory::InMemoryObjectStore;
    use std::os::unix::fs::symlink;

    let tempdir = tempfile::tempdir().unwrap();
    let repo = tempdir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let victim = tempdir.path().join("victim.txt");
    std::fs::write(&victim, b"PRISTINE").unwrap();

    let mut store = InMemoryObjectStore::default();
    let id = store.insert(b"tracked content").unwrap();

    // Case 1: a final-component symlink where a tracked file belongs.
    symlink(&victim, repo.join("data.txt")).unwrap();
    let mut root = BTreeMap::new();
    root.insert("data.txt".to_string(), DirectoryEntry::File(id));
    Directory { root }.write(&store, &repo, false).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"PRISTINE", "wrote through symlink");
    assert!(!repo.join("data.txt").symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(repo.join("data.txt")).unwrap(), b"tracked content");

    // Case 2: a leading-directory symlink pointing outside the repo.
    let outside = tempdir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("f.txt"), b"PRISTINE").unwrap();
    symlink(&outside, repo.join("d")).unwrap();
    let mut inner = BTreeMap::new();
    inner.insert("f.txt".to_string(), DirectoryEntry::File(id));
    let mut outer = BTreeMap::new();
    outer.insert("d".to_string(), DirectoryEntry::Directory(Box::new(Directory { root: inner })));
    Directory { root: outer }.write(&store, &repo, false).unwrap();
    assert_eq!(
        std::fs::read(outside.join("f.txt")).unwrap(),
        b"PRISTINE",
        "wrote through a symlinked directory to outside the repo"
    );
    assert_eq!(std::fs::read(repo.join("d/f.txt")).unwrap(), b"tracked content");
}

#[test]
fn test_trailing_slash_ignore_matches_directory() {
    // gitignore's `build/` directory syntax must actually exclude the directory
    // (and its contents), not silently do nothing.
    let ignores = Ignores::new(vec!["build/".to_string()]);
    assert!(ignores.is_ignored(Path::new("build")));
    assert!(ignores.is_ignored(Path::new("/repo/build")));
    // A pattern that is only a slash normalizes away and matches nothing.
    let only_slash = Ignores::new(vec!["/".to_string()]);
    assert!(!only_slash.is_ignored(Path::new("anything")));
}

#[test]
fn test_reject_too_deeply_nested_tree() {
    // Build a tree nested past the limit and confirm building it errors rather
    // than producing a snapshot that later can't be read back.
    fn nest(depth: usize) -> Directory {
        if depth == 0 {
            let mut root = BTreeMap::new();
            root.insert("leaf".to_string(), DirectoryEntry::File(ObjectId::from(&b"x"[..])));
            Directory { root }
        } else {
            let mut root = BTreeMap::new();
            root.insert("a".to_string(), DirectoryEntry::Directory(Box::new(nest(depth - 1))));
            Directory { root }
        }
    }
    assert_eq!(nest(MAX_DIRECTORY_DEPTH).max_depth(), MAX_DIRECTORY_DEPTH);
    assert!(nest(MAX_DIRECTORY_DEPTH)
        .reject_if_too_deep::<crate::object_store::in_memory::InMemoryObjectStore>()
        .is_ok());
    assert!(matches!(
        nest(MAX_DIRECTORY_DEPTH + 1)
            .reject_if_too_deep::<crate::object_store::in_memory::InMemoryObjectStore>(),
        Err(Error::TooDeeplyNested { .. })
    ));
}
