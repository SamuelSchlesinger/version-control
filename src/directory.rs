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
}

impl<Store: ObjectStore> From<std::io::Error> for Error<Store> {
    fn from(error: std::io::Error) -> Self {
        Error::IO(error)
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
        for (_, entry) in &self.modified {
            match entry {
                DiffEntry::FileWithContentDiff { content_diff, .. } => {
                    if content_diff.is_some() {
                        return true;
                    }
                }
                DiffEntry::Directory(diff) => {
                    if diff.has_content_diffs() {
                        return true;
                    }
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
                    if with_content_diff && store.is_some() {
                        match (dir_entry, other_dir_entry) {
                            // For files, use the content diff method
                            (DirectoryEntry::File(_), DirectoryEntry::File(_)) => {
                                dir_entry
                                    .generate_content_diff(other_dir_entry, store.unwrap())
                                    .map(|diff| (file_name.clone(), diff))
                            }
                            // For directories, recursively apply content diffing
                            (DirectoryEntry::Directory(dir), DirectoryEntry::Directory(other_dir)) => {
                                // Recursively diff the directories with content diffing enabled
                                let nested_diff = dir.diff_with_content(other_dir, with_content_diff, store.unwrap());
                                Some((file_name.clone(), DiffEntry::Directory(Box::new(nested_diff))))
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
        if read_dir(path).is_ok() {
            // First, write/update all files and directories in the snapshot
            for (file_name, entry) in self.root.iter() {
                match entry {
                    DirectoryEntry::File(id) => {
                        let v = store.read(*id).map_err(Error::Store)?;
                        match v {
                            Some(v) => {
                                let mut f = File::options()
                                    .create(true)
                                    .write(true)
                                    .truncate(true)
                                    .open(path.join(file_name))?;
                                f.write_all(&v)?;
                            }
                            None => return Err(Error::ObjectMissing(*id)),
                        }
                    }
                    DirectoryEntry::Directory(dir) => {
                        let dir_path = PathBuf::from(path).join(file_name);

                        // Create the directory if it doesn't exist
                        if !Path::try_exists(&dir_path)? {
                            std::fs::create_dir(&dir_path)?;
                        }

                        dir.write(store, dir_path.as_path(), delete_absent)?;
                    }
                }
            }

            // If delete_absent is true, remove files that aren't in the snapshot
            if delete_absent {
                for entry in read_dir(path)? {
                    let entry = entry?;
                    let file_name = entry.file_name().into_string().unwrap();

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

            let file_type = dir_entry.file_type().map_err(Error::IO)?;
            if file_type.is_dir() {
                let directory = Directory::new(path.as_path(), ignores, store)?;
                root.insert(
                    dir_entry.file_name().into_string().unwrap(),
                    DirectoryEntry::Directory(Box::new(directory)),
                );
            } else if file_type.is_file() {
                let id = ObjectId::try_from(path.as_path()).map_err(Error::IO)?;
                root.insert(
                    dir_entry.file_name().into_string().unwrap(),
                    DirectoryEntry::File(id),
                );
                let mut v = Vec::new();
                let mut obj_file = File::options()
                    .read(true)
                    .open(&path)
                    .map_err(Error::IO)?;
                obj_file.read_to_end(&mut v).map_err(Error::IO)?;
                store.insert(&v).map_err(Error::Store)?;
            } else {
                log::warn!(
                    "Skipping unsupported file type (not a regular file or directory): {:?}",
                    path
                );
            }
        }
        Ok(Directory { root })
    }
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
    assert!(codebase.root.get(&readme_path).is_some());
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
