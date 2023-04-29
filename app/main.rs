use std::{
    collections::BTreeSet,
    env::current_dir,
    fmt::Debug,
    fs::{create_dir, read_dir, File},
    io::stdout,
    path::Path,
};

use clap::{Parser, Subcommand};
use lib::{
    directory::{Directory, Ignores},
    object_id::ObjectId,
    object_store::{directory::DirectoryObjectStore, ObjectStore},
    snapshot::SnapShot,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
struct Arguments {
    #[clap(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "initialize a brand new revision")]
    Init,
    #[clap(about = "shows the changed files or directories")]
    Changes,
    #[clap(about = "take a new snapshot")]
    Snap {
        #[arg(short, long, help = "message to leave with this snapshot")]
        message: String,
    },
}

pub fn read_json<A: for<'de> Deserialize<'de>>(path: &Path) -> A {
    serde_json::from_reader(File::options().read(true).open(path).unwrap()).unwrap()
}

pub fn write_json<A: Serialize>(thing: &A, path: &Path) {
    serde_json::to_writer_pretty(
        File::options().write(true).create(true).open(path).unwrap(),
        thing,
    )
    .unwrap()
}

fn main() {
    let args = Arguments::parse();
    use Command::*;
    match args.cmd {
        Changes => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
            }
            let mut store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let old_tip: ObjectId = read_json(&rev_dir.join("tip"));
            let ignores: Ignores = read_json(&rev_dir.join("ignores"));
            let directory = Directory::new(dir.as_path(), &ignores, &mut store).unwrap();
            let snapshot: SnapShot =
                serde_json::from_slice(&store.read(old_tip).expect("1").expect("2")).expect("3");
            let old_directory: Directory =
                serde_json::from_slice(&store.read(snapshot.directory).unwrap().unwrap()).unwrap();
            serde_json::to_writer_pretty(stdout(), &old_directory.diff(&directory)).unwrap();
        }
        Snap { message } => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
            }
            let mut store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let old_tip: ObjectId = read_json(&rev_dir.join("tip"));
            let ignores: Ignores = read_json(&rev_dir.join("ignores"));
            let directory = Directory::new(dir.as_path(), &ignores, &mut store).unwrap();
            let directory_id = store
                .insert(&serde_json::to_vec_pretty(&directory).unwrap())
                .unwrap();
            let snap = SnapShot {
                directory: directory_id,
                previous: vec![old_tip].into_iter().collect(),
                message,
            };
            let snap_id = store
                .insert(&serde_json::to_vec_pretty(&snap).unwrap())
                .unwrap();
            write_json(&snap_id, &rev_dir.join("tip"));
        }
        Init => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if read_dir(&rev_dir).is_ok() {
                return;
            }
            create_dir(&rev_dir).unwrap();
            let mut store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let ignores = Ignores {
                set: vec![
                    String::from(".git"),
                    String::from("target"),
                    String::from(".rev"),
                ]
                .into_iter()
                .collect(),
            };
            write_json(&ignores, &rev_dir.join("ignores"));
            let directory = Directory::new(dir.as_path(), &ignores, &mut store).unwrap();
            let directory_bytes = serde_json::to_vec_pretty(&directory).unwrap();
            let directory_id = store.insert(&directory_bytes).unwrap();
            let snapshot = SnapShot {
                directory: directory_id,
                message: String::from("init"),
                previous: BTreeSet::new(),
            };
            let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).unwrap();
            let snapshot_id = store.insert(&snapshot_bytes).unwrap();
            write_json(&snapshot_id, &rev_dir.join("tip"));
        }
    }
}
