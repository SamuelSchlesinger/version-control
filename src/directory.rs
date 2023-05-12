use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{read_dir, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{object_id::ObjectId, object_store::ObjectStore};

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

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Diff {
    pub deleted: BTreeSet<String>,
    pub added: BTreeMap<String, DirectoryEntry>,
    pub modified: BTreeMap<String, DiffEntry>,
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum DiffEntry {
    File(ObjectId),
    Directory(Box<Diff>),
}

impl DirectoryEntry {
    pub fn diff(&self, other: &DirectoryEntry) -> Option<DiffEntry> {
        use DirectoryEntry::*;
        match (self, other) {
            (File(id), File(id_)) => {
                if id != id_ {
                    Some(DiffEntry::File(*id_))
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
}

impl Directory {
    /// Compute the diff between this directory structure and the one
    /// which is currently located at the path.
    pub fn diff(&self, other: &Directory) -> Diff {
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
                    dir_entry
                        .diff(other_dir_entry)
                        .map(|diff| (file_name.clone(), diff))
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
    pub fn write<Store: ObjectStore>(
        &self,
        store: &Store,
        path: &Path,
    ) -> Result<(), Error<Store>> {
        if read_dir(path).is_ok() {
            for (file_name, entry) in self.root.iter() {
                match entry {
                    DirectoryEntry::File(id) => {
                        let v = store.read(*id).map_err(Error::Store)?;
                        match v {
                            Some(v) => {
                                let mut f = File::options()
                                    .create(true)
                                    .write(true)
                                    .open(path.join(file_name))
                                    .map_err(Error::IO)?;
                                f.write(&v).map_err(Error::IO)?;
                            }
                            None => return Err(Error::ObjectMissing(*id)),
                        }
                    }
                    DirectoryEntry::Directory(dir) => {
                        dir.write(store, PathBuf::from(path).join(file_name).as_path())?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// The set of file names which we will ignore at any level.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Ignores {
    pub set: BTreeSet<String>,
}

impl Default for Ignores {
    fn default() -> Self {
        Ignores {
            set: vec![String::from(".rev")].into_iter().collect(),
        }
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
            if ignores
                .set
                .contains(&dir_entry.file_name().into_string().unwrap())
            {
                continue;
            }
            let file_type = dir_entry.file_type().map_err(Error::IO)?;
            if file_type.is_dir() {
                let directory = Directory::new(dir_entry.path().as_path(), ignores, store)?;
                root.insert(
                    dir_entry.file_name().into_string().unwrap(),
                    DirectoryEntry::Directory(Box::new(directory)),
                );
            } else if file_type.is_file() {
                let id = ObjectId::try_from(dir_entry.path().as_path()).map_err(Error::IO)?;
                root.insert(
                    dir_entry.file_name().into_string().unwrap(),
                    DirectoryEntry::File(id),
                );
                let mut v = Vec::new();
                let mut obj_file = File::options()
                    .read(true)
                    .open(dir_entry.path())
                    .map_err(Error::IO)?;
                obj_file.read_to_end(&mut v).map_err(Error::IO)?;
                store.insert(&v).map_err(Error::Store)?;
            } else {
                eprintln!(
                    "TODO support things which aren't files or directories: {:?}",
                    dir_entry.file_name()
                );
            }
        }
        Ok(Directory { root })
    }
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

        enum DiffItem {
            Deleted,
            Added,
            Modified,
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
                    DiffEntry::Directory(diff) => {
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
    assert_eq!(
        diff_5.to_string(),
        [
            "D a/foo",
            "A bar",
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
    let codebase = Directory::new(
        dir.as_path(),
        &Ignores {
            set: vec![
                String::from(".git"),
                String::from(".rev"),
                String::from("target"),
            ]
            .into_iter()
            .collect(),
        },
        &mut store,
    )
    .unwrap();
    let readme_path = String::from("README.md");
    assert!(codebase.root.get(&readme_path).is_some());
}
