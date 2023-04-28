use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::object_id::ObjectId;

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct Directory {
    pub root: BTreeMap<OsString, DirectoryEntry>,
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

impl TryFrom<(&Path, &Ignores)> for Directory {
    type Error = std::io::Error;

    fn try_from((dir, ignores): (&Path, &Ignores)) -> Result<Self, Self::Error> {
        let mut root = BTreeMap::new();
        for f in std::fs::read_dir(dir)? {
            let dir_entry = f?;
            eprintln!("{}", dir_entry.file_name().to_str().unwrap());
            if ignores.set.contains(&dir_entry.file_name()) {
                continue;
            }
            let file_type = dir_entry.file_type()?;
            if file_type.is_dir() {
                let directory = Directory::try_from((dir_entry.path().as_path(), ignores))?;
                root.insert(
                    dir_entry.file_name().into(),
                    DirectoryEntry::Directory(Box::new(directory)),
                );
            } else if file_type.is_file() {
                let id = ObjectId::try_from(dir_entry.path().as_path())?;
                root.insert(dir_entry.file_name(), DirectoryEntry::File(id));
            } else {
                panic!("TODO support things which aren't files or directories");
            }
        }
        Ok(Directory { root })
    }
}

#[test]
fn test_directory() {
    use std::env::current_dir;
    let dir = current_dir().unwrap();
    let codebase = Directory::try_from((
        dir.as_path(),
        &Ignores {
            set: vec![OsString::from(".git"), OsString::from("target")]
                .into_iter()
                .collect(),
        },
    ))
    .unwrap();
    let readme_path = OsString::from("README.md");
    assert!(codebase.root.get(&readme_path).is_some());
}
