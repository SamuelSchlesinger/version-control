use std::{
    collections::BTreeSet,
    fs::{create_dir, create_dir_all, read_dir, read_to_string, try_exists, File},
    io::Write,
    path::{Path, PathBuf},
};

use derive_more::From;
use serde::{Deserialize, Serialize};

use crate::{
    directory::{Directory, Ignores},
    object_id::ObjectId,
    object_store::{directory::DirectoryObjectStore, ObjectStore},
    snapshot::SnapShot,
};

/// A wrapper for the path of the .rev directory which has a number of utilities defined on it.
pub struct DotRev {
    root: PathBuf,
}

#[derive(Debug, From)]
pub enum Error {
    #[from]
    IO(std::io::Error),
    #[from]
    Serde(serde_json::Error),
    MissingObject(ObjectId),
}
impl DotRev {
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn init(root: PathBuf) -> Result<Self, Error> {
        if read_dir(&root).is_ok() {
            return Ok(Self { root });
        }
        create_dir_all(&root)?;

        // Start out on the dev branch
        let mut file = File::options()
            .create(true)
            .write(true)
            .open(&root.join("branch"))?;
        file.write("dev".as_bytes())?;

        // Create the branches directory
        create_dir(&root.join("branches"))?;

        // Create the init commit on the dev branch
        let mut store = DirectoryObjectStore::new(root.join("store"))?;
        let directory = Directory::default();
        let directory = store.insert_json(&directory)?;
        let snapshot = SnapShot {
            directory,
            message: String::from("init"),
            previous: BTreeSet::new(),
        };
        let snapshot_id = store.insert_json(&snapshot)?;
        write_json(&snapshot_id, &root.join("branches").join("dev"))?;
        let ignores = Ignores::default();
        write_json(&ignores, &root.join("ignores"))?;

        Ok(DotRev { root })
    }

    pub fn existing(root: PathBuf) -> Result<Self, Error> {
        read_dir(&root)?;
        Ok(DotRev { root })
    }

    pub fn branch(&self) -> Result<String, Error> {
        Ok(read_to_string(&self.root.join("branch"))?)
    }

    pub fn set_branch(&self, new_branch: &str) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .open(&self.root.join("branch"))?;
        file.write(new_branch.as_bytes())?;
        Ok(())
    }

    pub fn branch_snapshot_id(&self, branch: &str) -> Result<ObjectId, Error> {
        read_json(&self.root.join("branches").join(&branch))
    }

    pub fn set_branch_snapshot_id(&self, branch: &str, object_id: ObjectId) -> Result<(), Error> {
        write_json(&object_id, &self.root.join("branches").join(&branch))
    }

    pub fn current_snapshot_id(&self) -> Result<ObjectId, Error> {
        let branch = self.branch()?;
        self.branch_snapshot_id(&branch)
    }

    pub fn create_branch(&self, new_branch: &str) -> Result<(), Error> {
        if !self.branch_exists(&new_branch)? {
            let snapshot_id = self.current_snapshot_id()?;
            return write_json(&snapshot_id, &self.root.join("branches").join(&new_branch));
        }
        Ok(())
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool, Error> {
        Ok(try_exists(self.root.join("branches").join(&branch))?)
    }

    pub fn store(&self) -> Result<DirectoryObjectStore, Error> {
        Ok(DirectoryObjectStore::new(self.root.clone())?)
    }

    pub fn ignores(&self) -> Result<Ignores, Error> {
        Ok(read_json(&self.root.join("ignores"))?)
    }
}

/// A convenience trait for writing and reading JSON from the [`DirectoryObjectStore`].
pub trait InsertJson {
    /// Inserts a pretty JSON encoded version of the thing into the store.
    fn insert_json<A: Serialize>(&mut self, thing: &A) -> Result<ObjectId, Error>;

    /// Reads a JSON encoded thing of the given type from the store at that given [`ObjectId`].
    fn read_json<A: for<'de> Deserialize<'de>>(&mut self, object_id: ObjectId) -> Result<A, Error>;
}

impl InsertJson for DirectoryObjectStore {
    fn insert_json<A: Serialize>(&mut self, thing: &A) -> Result<ObjectId, Error> {
        Ok(self.insert(&serde_json::to_vec_pretty(thing)?)?)
    }

    fn read_json<A: for<'de> Deserialize<'de>>(&mut self, object_id: ObjectId) -> Result<A, Error> {
        match self.read(object_id)? {
            None => Err(Error::MissingObject(object_id)),
            Some(obj) => Ok(serde_json::from_slice(&obj)?),
        }
    }
}

fn read_json<A: for<'de> Deserialize<'de>>(path: &Path) -> Result<A, Error> {
    Ok(serde_json::from_reader(
        File::options().read(true).open(path)?,
    )?)
}

fn write_json<A: Serialize>(thing: &A, path: &Path) -> Result<(), Error> {
    Ok(serde_json::to_writer_pretty(
        File::options().write(true).create(true).open(path)?,
        thing,
    )?)
}
