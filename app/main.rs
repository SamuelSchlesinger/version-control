#![feature(fs_try_exists)]

use std::{
    collections::BTreeSet,
    env::current_dir,
    fmt::Debug,
    fs::{create_dir, read_dir, try_exists, File},
    io::stdout,
    path::Path,
    process::exit,
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
    #[clap(about = "check the difference between this branch and another")]
    Diff { branch: String },
    #[clap(
        about = "shows the files and directories which have been changed since the latest snap"
    )]
    Changes,
    #[clap(about = "take a new snapshot")]
    Snap {
        #[arg(short, long, help = "message to leave with this snapshot")]
        message: String,
    },
    #[clap(about = "switch to branch")]
    Checkout {
        #[arg(short, long, help = "branch to checkout")]
        branch: String,
    },
    #[clap(about = "print out current branch")]
    Branch,
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
    env_logger::init();
    let args = Arguments::parse();
    use Command::*;
    match args.cmd {
        Diff { branch } => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
                exit(1);
            }
            let store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let that_branch = branch;
            let this_branch: String = read_json(&rev_dir.join("branch"));
            let branch_dir = rev_dir.join("branches");
            if !try_exists(branch_dir.as_path().join(&that_branch)).unwrap() {
                eprintln!("no branch named {} exists", that_branch);
                exit(1);
            }
            let this_tip: ObjectId = read_json(&branch_dir.join(&this_branch));
            // let ignores: Ignores = read_json(&rev_dir.join("ignores"));
            let that_tip: ObjectId = read_json(&branch_dir.join(&that_branch));
            let that_snapshot: SnapShot =
                serde_json::from_slice(&store.read(that_tip).expect("a").expect("b")).expect("c");
            let that_branch_directory = serde_json::from_slice(
                &store.read(that_snapshot.directory).expect("x").expect("y"),
            )
            .expect("z");
            let this_snapshot: SnapShot =
                serde_json::from_slice(&store.read(this_tip).expect("1").expect("2")).expect("3");
            let this_branch_directory: Directory =
                serde_json::from_slice(&store.read(this_snapshot.directory).unwrap().unwrap())
                    .unwrap();
            serde_json::to_writer_pretty(
                stdout(),
                &this_branch_directory.diff(&that_branch_directory),
            )
            .unwrap();
        }
        Branch => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
                exit(1);
            }
            let branch: String = read_json(&rev_dir.join("branch"));
            println!("{}", branch);
        }
        Checkout { branch } => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
                exit(1);
            }
            let branch_dir = rev_dir.join("branches");
            if !try_exists(&branch_dir.join(&branch)).unwrap() {
                let branch: String = read_json(&rev_dir.join("branch"));
                let old_tip: ObjectId = read_json(&branch_dir.join(&branch));
                write_json(&old_tip, &branch_dir.join(&branch));
            }
            write_json(&branch, &rev_dir.join("branch"));
        }
        Changes => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if !read_dir(&rev_dir).is_ok() {
                eprintln!("no .rev in working directory");
                exit(1);
            }
            let mut store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let branch: String = read_json(&rev_dir.join("branch"));
            let branch_dir = rev_dir.join("branches");
            let old_tip: ObjectId = read_json(&branch_dir.join(&branch));
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
            let branch: String = read_json(&rev_dir.join("branch"));
            let branch_dir = rev_dir.join("branches");
            let old_tip: ObjectId = read_json(&branch_dir.join(&branch));
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
            write_json(&snap_id, &branch_dir.join(&branch));
        }
        Init => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            if read_dir(&rev_dir).is_ok() {
                return;
            }
            create_dir(&rev_dir).unwrap();
            let mut store = DirectoryObjectStore::new(rev_dir.join("store")).unwrap();
            let ignores = Ignores::default();
            write_json(&ignores, &rev_dir.join("ignores"));
            let directory = Directory::default();
            let directory_bytes = serde_json::to_vec_pretty(&directory).unwrap();
            let directory_id = store.insert(&directory_bytes).unwrap();
            let snapshot = SnapShot {
                directory: directory_id,
                message: String::from("empty!"),
                previous: BTreeSet::new(),
            };
            let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).unwrap();
            let snapshot_id = store.insert(&snapshot_bytes).unwrap();
            let branch_dir = rev_dir.as_path().join("branches");
            create_dir(&branch_dir).unwrap();
            write_json(&snapshot_id, &branch_dir.as_path().join("dev"));
            write_json(&String::from("dev"), &rev_dir.join("branch"));
        }
    }
}
