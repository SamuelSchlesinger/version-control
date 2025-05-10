use std::{env::current_dir, fmt::Debug, io::stdout, process::exit};

use clap::{Parser, Subcommand};
use lib::{
    directory::{Directory, Ignores},
    dot_rev::{DotRev, Error as DotRevError, InsertJson},
    object_id::ObjectId,
    snapshot::SnapShot,
};

// Application error type
#[derive(Debug)]
enum AppError {
    DotRevError(DotRevError),
    IoError(std::io::Error),
    Other(String),
}

impl From<DotRevError> for AppError {
    fn from(err: DotRevError) -> Self {
        AppError::DotRevError(err)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::IoError(err)
    }
}

impl From<String> for AppError {
    fn from(err: String) -> Self {
        AppError::Other(err)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::DotRevError(err) => write!(f, "Repository error: {:?}", err),
            AppError::IoError(err) => write!(f, "I/O error: {}", err),
            AppError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

type AppResult<T> = Result<T, AppError>;

#[derive(Parser, Debug)]
struct Arguments {
    #[clap(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "Initialize a brand new revision")]
    Init,

    #[clap(about = "Check the difference between this branch and another")]
    Diff {
        #[arg(help = "Branch to compare with current branch")]
        branch: String
    },

    #[clap(about = "Shows files and directories changed since the latest snapshot")]
    Changes,

    #[clap(about = "Show working tree status")]
    Status,

    #[clap(about = "Take a new snapshot (similar to git commit)")]
    Snap {
        #[arg(short, long, help = "Message to leave with this snapshot")]
        message: String,
    },

    #[clap(about = "Switch to a branch")]
    Checkout {
        #[arg(help = "Branch to checkout")]
        branch: String,
    },

    #[clap(about = "Create a new branch")]
    Branch {
        #[arg(help = "Name of the new branch")]
        name: Option<String>,
    },

    #[clap(about = "Reset all files to the last snapshot on this branch")]
    Reset {
        #[arg(
            short,
            long,
            default_value = "false",
            help = "Whether to delete files absent from the snapshot"
        )]
        delete_absent: bool,
    },

    #[clap(about = "Show commit logs")]
    Log {
        #[arg(short, long, default_value = "10", help = "Number of commits to show")]
        limit: usize,
    },
}

// A helper function to get the repository
fn get_repository() -> AppResult<(DotRev, String)> {
    let dot_rev = DotRev::here()?;
    let branch = dot_rev.branch()?;
    Ok((dot_rev, branch))
}

fn run_command(cmd: Command) -> AppResult<()> {
    use Command::*;
    match cmd {
        Reset { delete_absent } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let snapshot_id = dot_rev.branch_snapshot_id(&branch)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let directory: Directory = store.read_json(snapshot.directory)?;

            // Restore files from snapshot
            let cwd = current_dir()?;
            directory.write(&store, &cwd, delete_absent)
                .map_err(|e| AppError::Other(format!("Failed to reset files: {:?}", e)))?;

            println!("Reset to the last snapshot on branch '{}' ({})", branch, snapshot_id);
            Ok(())
        }
        Diff { branch } => {
            let (dot_rev, this_branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let that_branch = branch;

            if !dot_rev.branch_exists(&that_branch)? {
                return Err(AppError::Other(format!("Branch '{}' does not exist", that_branch)));
            }

            // Load snapshots for both branches
            let this_tip = dot_rev.branch_snapshot_id(&this_branch)?;
            let that_tip = dot_rev.branch_snapshot_id(&that_branch)?;

            // Load directory structures
            let this_snapshot: SnapShot = store.read_json(this_tip)?;
            let that_snapshot: SnapShot = store.read_json(that_tip)?;

            let this_branch_directory: Directory = store.read_json(this_snapshot.directory)?;
            let that_branch_directory: Directory = store.read_json(that_snapshot.directory)?;

            // Calculate and display diff
            let diff = &this_branch_directory.diff(&that_branch_directory);
            println!("Diff between branch '{}' and '{}':", this_branch, that_branch);
            println!("{diff}");

            Ok(())
        }
        Branch { name } => {
            let (dot_rev, current_branch) = get_repository()?;

            match name {
                Some(branch_name) => {
                    // Create a new branch
                    dot_rev.create_branch(&branch_name)?;
                    println!("Created branch '{}'", branch_name);
                    Ok(())
                },
                None => {
                    // List branches and show current
                    let branches = dot_rev.list_branches()?;

                    for branch in branches {
                        if branch == current_branch {
                            println!("* {}", branch); // Current branch marked with asterisk
                        } else {
                            println!("  {}", branch);
                        }
                    }
                    Ok(())
                }
            }
        },

        Status => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let old_tip: ObjectId = dot_rev.branch_snapshot_id(&branch)?;
            let ignores: Ignores = dot_rev.ignores()?;
            let cwd = current_dir()?;
            let directory = Directory::new(cwd.as_path(), &ignores, &mut store)
                .map_err(|e| AppError::Other(format!("Failed to read current directory: {:?}", e)))?;
            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;
            let diff = old_directory.diff(&directory);

            println!("On branch {}", branch);
            if diff.added.is_empty() && diff.deleted.is_empty() && diff.modified.is_empty() {
                println!("Working tree clean, nothing to snapshot");
            } else {
                println!("\nChanges not yet snapped:");
                println!("  (use \"revtool snap -m <message>\" to create a new snapshot)\n");

                for file in diff.added.keys() {
                    println!("        new file: {}", file);
                }

                for file in &diff.deleted {
                    println!("        deleted: {}", file);
                }

                for file in diff.modified.keys() {
                    println!("        modified: {}", file);
                }
            }
            Ok(())
        },

        Log { limit } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let mut snapshot_id = dot_rev.branch_snapshot_id(&branch)?;

            println!("Commit history for branch '{}':", branch);
            println!("--------------------------------");

            let mut count = 0;
            loop {
                if count >= limit {
                    break;
                }

                let snapshot: SnapShot = store.read_json(snapshot_id)?;
                println!("Snapshot: {}", snapshot_id);
                println!("Message: {}", snapshot.message);
                println!("--------------------------------");

                count += 1;

                // Move to previous commit
                if snapshot.previous.is_empty() {
                    break;
                }

                // Just take the first parent for now
                snapshot_id = *snapshot.previous.iter().next().unwrap();
            }
            Ok(())
        }
        Checkout { branch } => {
            let (dot_rev, current_branch) = get_repository()?;
            let mut store = dot_rev.store()?;

            // Don't do anything if trying to checkout the current branch
            if branch == current_branch {
                println!("Already on branch '{}'", branch);
                return Ok(());
            }

            // Create the branch if it doesn't exist
            if !dot_rev.branch_exists(&branch)? {
                println!("Creating new branch '{}'", branch);
                dot_rev.create_branch(&branch)?;
            }

            // Switch to the branch
            dot_rev.set_branch(&branch)?;

            // Reset to the snapshot on this branch
            let snapshot_id = dot_rev.branch_snapshot_id(&branch)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let directory: Directory = store.read_json(snapshot.directory)?;

            // Restore files from snapshot (without deleting files not in snapshot)
            let cwd = current_dir()?;
            directory.write(&store, &cwd, false)
                .map_err(|e| AppError::Other(format!("Failed to restore files: {:?}", e)))?;

            println!("Switched to branch '{}' (snapshot: {})", branch, snapshot_id);
            Ok(())
        }
        Changes => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let old_tip = dot_rev.branch_snapshot_id(&branch)?;
            let ignores = dot_rev.ignores()?;

            let directory = Directory::new(
                current_dir()?.as_path(),
                &ignores,
                &mut store
            ).map_err(|e| AppError::Other(format!("Failed to read current directory: {:?}", e)))?;

            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;

            serde_json::to_writer_pretty(stdout(), &old_directory.diff(&directory))
                .map_err(|e| AppError::Other(format!("Failed to output changes: {}", e)))?;
            Ok(())
        }
        Snap { message } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let old_tip = dot_rev.branch_snapshot_id(&branch)?;
            let ignores = dot_rev.ignores()?;

            // Create a snapshot of the current directory
            let directory = Directory::new(
                current_dir()?.as_path(),
                &ignores,
                &mut store
            ).map_err(|e| AppError::Other(format!("Failed to read current directory: {:?}", e)))?;

            // Check if there are any changes
            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;
            let diff = old_directory.diff(&directory);

            if diff.added.is_empty() && diff.deleted.is_empty() && diff.modified.is_empty() {
                println!("No changes to record in snapshot");
                return Ok(());
            }

            // Store the new directory and create a snapshot
            let directory_id = store.insert_json(&directory)?;
            let snap = SnapShot {
                directory: directory_id,
                previous: vec![old_tip].into_iter().collect(),
                message,
            };

            // Store the snapshot and update the branch
            let snap_id = store.insert_json(&snap)?;
            dot_rev.set_branch_snapshot_id(&branch, snap_id)?;

            println!("Created snapshot {} on branch '{}'", snap_id, branch);
            Ok(())
        }
        Init => {
            DotRev::init(current_dir()?.join(".rev"))?;
            println!("Initialized empty revision control repository in .rev/");
            Ok(())
        }
    }
}

fn main() {
    env_logger::init();
    let args = Arguments::parse();

    if let Err(err) = run_command(args.cmd) {
        eprintln!("Error: {}", err);
        exit(1);
    }
}
