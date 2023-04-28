use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{read_dir, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{object_id::ObjectId, object_store::ObjectStore};

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Directory {
    pub root: BTreeMap<OsString, DirectoryEntry>,
}

#[derive(Debug)]
pub enum Error<Store: ObjectStore> {
    ObjectMissing(ObjectId),
    Store(Store::Error),
    IO(std::io::Error),
}

impl Directory {
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

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Ignores {
    set: BTreeSet<OsString>,
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
            eprintln!("{}", dir_entry.file_name().to_str().unwrap());
            if ignores.set.contains(&dir_entry.file_name()) {
                continue;
            }
            let file_type = dir_entry.file_type().map_err(Error::IO)?;
            if file_type.is_dir() {
                let directory = Directory::new(dir_entry.path().as_path(), ignores, store)?;
                root.insert(
                    dir_entry.file_name().into(),
                    DirectoryEntry::Directory(Box::new(directory)),
                );
            } else if file_type.is_file() {
                let id = ObjectId::try_from(dir_entry.path().as_path()).map_err(Error::IO)?;
                root.insert(dir_entry.file_name(), DirectoryEntry::File(id));
                let mut v = Vec::new();
                let mut obj_file = File::options()
                    .read(true)
                    .open(dir_entry.path())
                    .map_err(Error::IO)?;
                obj_file.read_to_end(&mut v).map_err(Error::IO)?;
                store.insert(&v).map_err(Error::Store)?;
            } else {
                panic!("TODO support things which aren't files or directories");
            }
        }
        Ok(Directory { root })
    }
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
            set: vec![OsString::from(".git"), OsString::from("target")]
                .into_iter()
                .collect(),
        },
        &mut store,
    )
    .unwrap();
    let readme_path = OsString::from("README.md");
    assert!(codebase.root.get(&readme_path).is_some());
}