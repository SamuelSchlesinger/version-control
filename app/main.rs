#![feature(fs_try_exists)]

use std::{env::current_dir, fmt::Debug, io::stdout, process::exit};

use clap::{Parser, Subcommand};
use lib::{
    directory::{Directory, Ignores},
    dot_rev::{DotRev, InsertJson},
    object_id::ObjectId,
    snapshot::SnapShot,
};

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

fn main() {
    env_logger::init();
    let args = Arguments::parse();
    use Command::*;
    match args.cmd {
        Diff { branch } => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            let dot_rev = DotRev::existing(rev_dir).unwrap();
            let mut store = dot_rev.store().unwrap();
            let that_branch = branch;
            let this_branch: String = dot_rev.branch().unwrap();
            if !dot_rev.branch_exists(&that_branch).unwrap() {
                eprintln!("no branch named {} exists", that_branch);
                exit(1);
            }
            let this_tip: ObjectId = dot_rev.branch_snapshot_id(&this_branch).unwrap();
            let that_tip: ObjectId = dot_rev.branch_snapshot_id(&that_branch).unwrap();
            let that_snapshot: SnapShot = store.read_json(that_tip).expect("read that tip");
            let that_branch_directory = store
                .read_json(that_snapshot.directory)
                .expect("read that directory");
            let this_snapshot: SnapShot = store.read_json(this_tip).expect("read this tip");
            let this_branch_directory: Directory = store
                .read_json(this_snapshot.directory)
                .expect("read this branch directory");
            // TODO Format this more nicely, this is cringe
            serde_json::to_writer_pretty(
                stdout(),
                &this_branch_directory.diff(&that_branch_directory),
            )
            .unwrap();
        }
        Branch => {
            let dot_rev = DotRev::existing(current_dir().unwrap().join(".rev")).unwrap();
            let branch = dot_rev.branch().unwrap();
            println!("{}", branch);
        }
        Checkout { branch } => {
            let dot_rev = DotRev::existing(current_dir().unwrap().join(".rev")).unwrap();
            if !dot_rev.branch_exists(&branch).unwrap() {
                dot_rev.create_branch(&branch).unwrap();
            }
            dot_rev.set_branch(&branch).unwrap();
        }
        Changes => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            let dot_rev = DotRev::existing(rev_dir).unwrap();
            let mut store = dot_rev.store().unwrap();
            let branch: String = dot_rev.branch().unwrap();
            let old_tip: ObjectId = dot_rev.branch_snapshot_id(&branch).unwrap();
            let ignores: Ignores = dot_rev.ignores().unwrap();
            let directory = Directory::new(dir.as_path(), &ignores, &mut store).unwrap();
            let snapshot: SnapShot = store.read_json(old_tip).unwrap();
            let old_directory: Directory = store.read_json(snapshot.directory).unwrap();
            serde_json::to_writer_pretty(stdout(), &old_directory.diff(&directory)).unwrap();
        }
        Snap { message } => {
            let dir = current_dir().unwrap();
            let rev_dir = dir.join(".rev");
            let dot_rev = DotRev::existing(rev_dir).unwrap();
            let mut store = dot_rev.store().unwrap();
            let branch: String = dot_rev.branch().unwrap();
            let old_tip: ObjectId = dot_rev.branch_snapshot_id(&branch).unwrap();
            let ignores: Ignores = dot_rev.ignores().unwrap();
            let directory = Directory::new(dir.as_path(), &ignores, &mut store).unwrap();
            let directory_id = store.insert_json(&directory).unwrap();
            let snap = SnapShot {
                directory: directory_id,
                previous: vec![old_tip].into_iter().collect(),
                message,
            };
            let snap_id = store.insert_json(&snap).unwrap();
            dot_rev.set_branch_snapshot_id(&branch, snap_id).unwrap();
        }
        Init => {
            DotRev::init(current_dir().unwrap().join(".rev")).unwrap();
        }
    }
}
