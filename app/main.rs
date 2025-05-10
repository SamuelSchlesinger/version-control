use std::{env::current_dir, fmt::Debug, io::stdout, process::exit};

use clap::{Parser, Subcommand};
use colored::*;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
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
    BranchNotFound(String),
    NoChangesToSnapshot,
    #[allow(dead_code)]
    DirectoryError(String),
    #[allow(dead_code)]
    MissingObject(ObjectId),
    FailedToReadDirectory(String),
    FailedToResetFiles(String),
    FailedToRestoreFiles(String),
    FailedToOutputChanges(String),
    RemoteNotFound(String),
    #[allow(dead_code)]
    RemoteAlreadyExists(String),
    #[allow(dead_code)]
    RemoteError(String),
    PushError(String),
    PullError(String),
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
            AppError::DotRevError(err) => match err {
                DotRevError::IO(io_err) => write!(f, "Repository I/O error: {}. Ensure you have proper permissions.", io_err),
                DotRevError::Serde(serde_err) => write!(f, "Repository data error: {}. The repository metadata might be corrupted.", serde_err),
                DotRevError::MissingObject(id) => write!(f, "Repository object missing: {}. Try running 'revtool reset' to restore consistency.", id),
                DotRevError::BranchNotFound(branch) => write!(f, "Branch '{}' not found. Use 'revtool branch' to list available branches.", branch),
                DotRevError::RepositoryNotInitialized => write!(f, "Repository not initialized. Use 'revtool init' first to create a repository."),
                DotRevError::CorruptRepository(msg) => write!(f, "Corrupt repository: {}. Consider reinitializing or restoring from backup.", msg),
                DotRevError::RemoteNotFound(name) => write!(f, "Remote '{}' not found. Use 'revtool remote list' to see available remotes.", name),
                DotRevError::RemoteAlreadyExists(name) => write!(f, "Remote '{}' already exists. Use 'revtool remote remove {}' first if you want to recreate it.", name, name),
                DotRevError::PushError(msg) => write!(f, "Push failed: {}. Check network connection and remote repository status.", msg),
                DotRevError::PullError(msg) => write!(f, "Pull failed: {}. Check network connection and remote repository status.", msg),
            },
            AppError::IoError(err) => {
                match err.kind() {
                    std::io::ErrorKind::NotFound => write!(f, "File not found: {}. Check the path and try again.", err),
                    std::io::ErrorKind::PermissionDenied => write!(f, "Permission denied: {}. Check your file permissions.", err),
                    std::io::ErrorKind::ConnectionRefused => write!(f, "Connection refused: {}. Check your network connection and try again.", err),
                    _ => write!(f, "I/O error: {}. Check file system status and permissions.", err),
                }
            },
            AppError::BranchNotFound(branch) => write!(f, "Branch '{}' does not exist. Use 'revtool branch' to list existing branches or create this branch first.", branch),
            AppError::NoChangesToSnapshot => write!(f, "No changes to record in snapshot. Make changes to files before creating a snapshot."),
            AppError::DirectoryError(msg) => write!(f, "Directory error: {}. Check directory permissions and structure.", msg),
            AppError::MissingObject(id) => write!(f, "Object '{}' missing from repository. Repository may be corrupted or incomplete.", id),
            AppError::FailedToReadDirectory(reason) => write!(f, "Failed to read current directory: {}. Check permissions and retry.", reason),
            AppError::FailedToResetFiles(reason) => write!(f, "Failed to reset files: {}. Ensure you have write permissions in the directory.", reason),
            AppError::FailedToRestoreFiles(reason) => write!(f, "Failed to restore files: {}. Check directory permissions and available space.", reason),
            AppError::FailedToOutputChanges(reason) => write!(f, "Failed to output changes: {}. Check terminal and stdout status.", reason),
            AppError::RemoteNotFound(name) => write!(f, "Remote '{}' not found. Use 'revtool remote list' to see available remotes or 'revtool remote add' to create one.", name),
            AppError::RemoteAlreadyExists(name) => write!(f, "Remote '{}' already exists. Use a different name or remove the existing remote first.", name),
            AppError::RemoteError(msg) => write!(f, "Remote operation failed: {}. Check network connection and remote status.", msg),
            AppError::PushError(msg) => write!(f, "Push failed: {}. Ensure the remote repository is accessible and you have proper permissions.", msg),
            AppError::PullError(msg) => write!(f, "Pull failed: {}. Check network connection and ensure the remote repository is available.", msg),
            AppError::Other(msg) => write!(f, "{}. Please check your command and try again.", msg),
        }
    }
}

type AppResult<T> = Result<T, AppError>;

#[derive(Subcommand, Debug)]
enum RemoteCommand {
    #[clap(about = "Add a new remote")]
    Add {
        #[arg(help = "Name of the remote")]
        name: String,

        #[arg(help = "URL or path to the remote repository")]
        url: String,
    },

    #[clap(about = "Remove a remote")]
    Remove {
        #[arg(help = "Name of the remote to remove")]
        name: String,
    },

    #[clap(about = "List all remotes")]
    List,
}

#[derive(Parser, Debug)]
#[clap(
    name = "revtool",
    about = "A lightweight version control system",
    version,
    long_about = "A lightweight version control system implemented in Rust that provides basic version control functionality"
)]
struct Arguments {
    #[clap(subcommand)]
    cmd: Command,

    #[clap(short, long, global = true, help = "Use interactive mode with prompts and confirmations")]
    interactive: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(
        about = "Add a remote repository reference",
        long_about = "Adds a reference to a remote repository that can be used for push/pull operations",
        after_help = "Example:\n  revtool remote add origin /path/to/remote/repo"
    )]
    Remote {
        #[clap(subcommand)]
        cmd: RemoteCommand,
    },

    #[clap(
        about = "Push changes to a remote repository",
        long_about = "Pushes local branch changes to a remote repository",
        after_help = "Example:\n  revtool push origin dev"
    )]
    Push {
        #[arg(help = "Name of the remote to push to")]
        remote: Option<String>,

        #[arg(help = "Branch to push")]
        branch: Option<String>,
    },

    #[clap(
        about = "Pull changes from a remote repository",
        long_about = "Pulls remote branch changes to the local repository",
        after_help = "Example:\n  revtool pull origin dev"
    )]
    Pull {
        #[arg(help = "Name of the remote to pull from")]
        remote: Option<String>,

        #[arg(help = "Branch to pull")]
        branch: Option<String>,
    },
    #[clap(
        about = "Display help information and usage examples",
        long_about = "Shows detailed help information and common usage examples for revtool commands",
        after_help = "Example:\n  revtool usage\n  revtool usage snap"
    )]
    Usage {
        #[arg(help = "Command to get help for")]
        command: Option<String>,
    },

    #[clap(
        about = "Manage the ignore patterns",
        long_about = "List, add, or remove ignore patterns for files that should be excluded from snapshots",
        after_help = "Examples:\n  revtool ignore                # List all patterns\n  revtool ignore \"**/*.log\"      # Add a pattern\n  revtool ignore --remove target  # Remove a pattern"
    )]
    Ignore {
        #[arg(help = "Pattern to add to ignore list")]
        pattern: Option<String>,

        #[arg(long, help = "Remove this pattern from the ignore list")]
        remove: bool,
    },
    #[clap(
        about = "Initialize a brand new revision control repository",
        long_about = "Creates a new repository in the current directory by creating a .rev directory which will be used to track revisions",
        after_help = "Example:\n  revtool init"
    )]
    Init,

    #[clap(
        about = "Check the difference between this branch and another",
        long_about = "Compares the files in the current branch with the specified branch and displays the differences",
        after_help = "Example:\n  revtool diff dev\n  revtool diff main"
    )]
    Diff {
        #[arg(help = "Branch to compare with current branch")]
        branch: String
    },

    #[clap(
        about = "Shows files and directories changed since the latest snapshot",
        long_about = "Outputs detailed information about which files have been added, modified, or deleted since the last snapshot",
        after_help = "Example:\n  revtool changes"
    )]
    Changes,

    #[clap(
        about = "Show working tree status",
        long_about = "Shows which files have been modified, added, or deleted since the last snapshot. Similar to 'git status'",
        after_help = "Example:\n  revtool status"
    )]
    Status,

    #[clap(
        about = "Take a new snapshot (similar to git commit)",
        long_about = "Records a new snapshot of the current directory state with the given message",
        after_help = "Example:\n  revtool snap -m \"Add new feature\"\n  revtool snap -m \"Fix bug in directory.rs\""
    )]
    Snap {
        #[arg(short, long, help = "Message to leave with this snapshot")]
        message: Option<String>,
    },

    #[clap(
        about = "Switch to a branch",
        long_about = "Changes the current branch to the specified branch and updates the working directory to match",
        after_help = "Example:\n  revtool checkout dev\n  revtool checkout main"
    )]
    Checkout {
        #[arg(help = "Branch to checkout")]
        branch: Option<String>,
    },

    #[clap(
        about = "Create or list branches",
        long_about = "Without arguments, lists all branches. With a name argument, creates a new branch",
        after_help = "Examples:\n  revtool branch             # List all branches\n  revtool branch new-feature  # Create a branch named 'new-feature'"
    )]
    Branch {
        #[arg(help = "Name of the new branch")]
        name: Option<String>,
    },

    #[clap(
        about = "Reset all files to the last snapshot on this branch",
        long_about = "Resets the working directory to match the latest snapshot on the current branch",
        after_help = "Examples:\n  revtool reset              # Reset but keep untracked files\n  revtool reset --delete-absent  # Reset and delete untracked files"
    )]
    Reset {
        #[arg(
            short,
            long,
            default_value = "false",
            help = "Whether to delete files absent from the snapshot"
        )]
        delete_absent: bool,
    },

    #[clap(
        about = "Show commit logs",
        long_about = "Displays the history of snapshots taken on the current branch",
        after_help = "Examples:\n  revtool log         # Show 10 most recent snapshots\n  revtool log -l 5    # Show 5 most recent snapshots"
    )]
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

/// Interactive mode helper for remote selection
fn interactive_remote_selection(dot_rev: &DotRev) -> AppResult<Option<String>> {
    let remotes = dot_rev.remotes()
        .map_err(|e| AppError::DotRevError(e))?;

    if remotes.remotes.is_empty() {
        return Err(AppError::RemoteNotFound("No remotes configured. Use 'revtool remote add' first.".to_string()));
    }

    let remote_names: Vec<String> = remotes.remotes.keys().cloned().collect();

    println!("\n{}", "Available remotes:".cyan().bold());

    let theme = ColorfulTheme::default();
    let selection = Select::with_theme(&theme)
        .with_prompt("Select remote")
        .default(0)
        .items(&remote_names)
        .interact()
        .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

    Ok(Some(remote_names[selection].clone()))
}

/// Interactive mode helper for branch selection
fn interactive_branch_selection(dot_rev: &DotRev, current_branch: &str) -> AppResult<Option<String>> {
    let branches = dot_rev.list_branches()?;

    if branches.is_empty() {
        return Err(AppError::Other("No branches found".to_string()));
    }

    println!("\n{}", "Available branches:".cyan().bold());

    let theme = ColorfulTheme::default();
    let selection = Select::with_theme(&theme)
        .with_prompt("Select branch")
        .default(branches.iter().position(|b| b == current_branch).unwrap_or(0))
        .items(&branches)
        .interact()
        .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

    Ok(Some(branches[selection].clone()))
}

/// Interactive mode helper to prompt for a snapshot message
fn interactive_snapshot_message(diff: &lib::directory::Diff) -> AppResult<Option<String>> {
    // Show the changes that will be included in this snapshot
    println!("\n{}", "Changes to be snapped:".yellow().bold());

    for file in diff.added.keys() {
        println!("        {}: {}", "new file".green().bold(), file);
    }

    for file in &diff.deleted {
        println!("        {}: {}", "deleted".red().bold(), file);
    }

    for file in diff.modified.keys() {
        println!("        {}: {}", "modified".yellow().bold(), file);
    }

    println!();

    // Prompt for the snapshot message
    let theme = ColorfulTheme::default();
    let msg: String = Input::with_theme(&theme)
        .with_prompt("Enter snapshot message")
        .validate_with(|input: &String| -> Result<(), &str> {
            if input.trim().is_empty() {
                Err("Message cannot be empty")
            } else {
                Ok(())
            }
        })
        .interact_text()
        .unwrap_or_else(|_| "Snapshot".to_string());

    // Confirm the snapshot
    if !Confirm::with_theme(&theme)
        .with_prompt("Create snapshot with these changes?")
        .default(true)
        .interact()
        .unwrap_or(false) {
        println!("Snapshot aborted.");
        return Ok(None);
    }

    Ok(Some(msg))
}

// Define help texts for command examples
pub fn get_command_help(command: &str) -> Option<String> {
    match command.to_lowercase().as_str() {
        "remote" => Some(r#"
Manage remote repositories:
  revtool remote add <name> <url>      # Add a new remote
  revtool remote remove <name>         # Remove a remote
  revtool remote list                  # List all remotes

Examples:
  revtool remote add origin /path/to/remote/repo
  revtool remote list"#.to_string()),

        "push" => Some(r#"
Push local changes to a remote repository:
  revtool push [<remote>] [<branch>]

Examples:
  revtool push                  # Push current branch to "origin" (interactive mode only)
  revtool push origin           # Push current branch to "origin"
  revtool push origin dev       # Push "dev" branch to "origin"

When using interactive mode (-i), you can select the remote and branch interactively."#.to_string()),

        "pull" => Some(r#"
Pull remote changes into local repository:
  revtool pull [<remote>] [<branch>]

Examples:
  revtool pull                  # Pull current branch from "origin" (interactive mode only)
  revtool pull origin           # Pull current branch from "origin"
  revtool pull origin dev       # Pull "dev" branch from "origin"

When using interactive mode (-i), you can select the remote and branch interactively."#.to_string()),
        "ignore" => Some(r#"
Manage files and directories to ignore:
  revtool ignore                # List all current ignore patterns
  revtool ignore <pattern>      # Add a new pattern to ignore
  revtool ignore --remove <pattern>  # Remove a pattern from ignore list

Examples:
  revtool ignore "**/*.log"    # Ignore all .log files in any directory
  revtool ignore "build/"      # Ignore the build directory
  revtool ignore "**/*.tmp"    # Ignore all .tmp files

Patterns support gitignore-style glob syntax:
  *      - Matches any sequence of non-separator characters
  **     - Matches any sequence of characters including separators
  ?      - Matches any single non-separator character
  [...]  - Matches character class
"#.to_string()),
        "init" => Some(r#"
Initialize a new revision control repository:
  revtool init

Creates a .rev directory in the current location and initializes a repository structure."#.to_string()),
        "diff" => Some(r#"
Compare changes between branches:
  revtool diff <branch>

Examples:
  revtool diff main        # Compare current branch with main

The diff command shows what changes would be merged if you merged the specified branch."#.to_string()),
        "status" => Some(r#"
Show the current status of the working directory:
  revtool status

Shows which files are modified, added, or deleted compared to the latest snapshot."#.to_string()),
        "snap" => Some(r#"
Create a new snapshot (like a commit):
  revtool snap -m "message"

Examples:
  revtool snap -m "Add login feature"
  revtool snap -m "Fix bug in error handling"

The message should be descriptive and explain what changes are included in the snapshot."#.to_string()),
        "checkout" => Some(r#"
Switch to a different branch:
  revtool checkout <branch>

Examples:
  revtool checkout main     # Switch to main branch
  revtool checkout dev      # Switch to dev branch
  revtool checkout feature  # Switch to feature branch

Will create the branch if it doesn't exist."#.to_string()),
        "branch" => Some(r#"
Create or list branches:
  revtool branch            # List all branches
  revtool branch <name>     # Create a new branch

Examples:
  revtool branch            # Show all branches
  revtool branch feature    # Create a new branch named "feature"

Creating a branch doesn't automatically switch to it. Use 'checkout' to switch."#.to_string()),
        "reset" => Some(r#"
Reset working directory to match the latest snapshot:
  revtool reset                   # Reset keeping untracked files
  revtool reset --delete-absent   # Reset and delete untracked files

Use with caution, especially with --delete-absent, as it will permanently delete files."#.to_string()),
        "log" => Some(r#"
Show history of snapshots:
  revtool log           # Show 10 most recent snapshots
  revtool log -l <num>  # Show <num> most recent snapshots

Examples:
  revtool log           # Show 10 most recent
  revtool log -l 5      # Show 5 most recent

Each snapshot shows its ID and commit message."#.to_string()),
        "changes" => Some(r#"
Show detailed information about changes since last snapshot:
  revtool changes

Shows a JSON representation of all changes in the working directory."#.to_string()),
        _ => None,
    }
}

pub fn show_general_help() -> String {
    r###"
revtool - A lightweight version control system

Common workflows:

1. Starting a new project:
   revtool init
   echo "# My Project" > README.md
   revtool snap -m "Initial commit"

2. Making changes:
   # make some changes to files
   revtool status            # See what's changed
   revtool snap -m "Message" # Create a snapshot

3. Working with branches:
   revtool branch feature    # Create a branch named "feature"
   revtool checkout feature  # Switch to the branch
   # make some changes
   revtool snap -m "Work on feature"

4. Comparing branches:
   revtool checkout main
   revtool diff feature      # See changes in feature compared to main

Use 'revtool usage <command>' for detailed help on a specific command.
"###.to_string()
}

fn run_command(cmd: Command, interactive: bool) -> AppResult<()> {
    use Command::*;
    use RemoteCommand::*;
    match cmd {
        Remote { cmd } => {
            let dot_rev = DotRev::here()?;

            match cmd {
                Add { name, url } => {
                    dot_rev.add_remote(&name, &url)
                        .map_err(|e| AppError::DotRevError(e))?;
                    println!("Added remote '{}' with URL '{}'", name.green().bold(), url);
                    Ok(())
                },
                Remove { name } => {
                    dot_rev.remove_remote(&name)
                        .map_err(|e| match e {
                            DotRevError::RemoteNotFound(name) => AppError::RemoteNotFound(name),
                            _ => AppError::DotRevError(e),
                        })?;
                    println!("Removed remote '{}'", name.red().bold());
                    Ok(())
                },
                List => {
                    let remotes = dot_rev.remotes()
                        .map_err(|e| AppError::DotRevError(e))?;

                    if remotes.remotes.is_empty() {
                        println!("No remotes configured.");
                    } else {
                        println!("{}", "Configured remotes:".cyan().bold());
                        for (name, remote) in remotes.remotes {
                            println!("  {} -> {}", name.green().bold(), remote.url);
                        }
                    }
                    Ok(())
                }
            }
        },

        Push { remote, branch } => {
            let (dot_rev, current_branch) = get_repository()?;

            // Get the remote name - either from command line or interactively
            let remote_name = if let Some(r) = remote {
                r
            } else if interactive {
                match interactive_remote_selection(&dot_rev)? {
                    Some(r) => r,
                    None => return Ok(()) // User aborted
                }
            } else {
                return Err(AppError::Other("No remote specified. Please provide a remote name or use interactive mode with -i".to_string()));
            };

            // Get the branch to push - either from command line or use current branch
            let branch_to_push = if let Some(b) = branch {
                b
            } else if interactive {
                match interactive_branch_selection(&dot_rev, &current_branch)? {
                    Some(b) => b,
                    None => return Ok(()) // User aborted
                }
            } else {
                current_branch
            };

            // Confirm the push if in interactive mode
            if interactive {
                let theme = ColorfulTheme::default();
                if !Confirm::with_theme(&theme)
                    .with_prompt(format!("Push branch '{}' to remote '{}'?", branch_to_push, remote_name))
                    .default(true)
                    .interact()
                    .unwrap_or(false) {
                    println!("Push aborted.");
                    return Ok(());
                }
            }

            // Perform the push
            println!("Pushing branch '{}' to remote '{}'...", branch_to_push.green().bold(), remote_name.green().bold());

            dot_rev.push(&remote_name, &branch_to_push)
                .map_err(|e| match e {
                    DotRevError::RemoteNotFound(name) => AppError::RemoteNotFound(name),
                    DotRevError::BranchNotFound(name) => AppError::BranchNotFound(name),
                    DotRevError::PushError(msg) => AppError::PushError(msg),
                    _ => AppError::DotRevError(e),
                })?;

            println!("Branch '{}' successfully pushed to remote '{}'", branch_to_push.green().bold(), remote_name.green().bold());
            Ok(())
        },

        Pull { remote, branch } => {
            let (dot_rev, current_branch) = get_repository()?;

            // Get the remote name - either from command line or interactively
            let remote_name = if let Some(r) = remote {
                r
            } else if interactive {
                match interactive_remote_selection(&dot_rev)? {
                    Some(r) => r,
                    None => return Ok(()) // User aborted
                }
            } else {
                return Err(AppError::Other("No remote specified. Please provide a remote name or use interactive mode with -i".to_string()));
            };

            // Get the branch to pull - either from command line or use current branch
            let branch_to_pull = if let Some(b) = branch {
                b
            } else if interactive {
                match interactive_branch_selection(&dot_rev, &current_branch)? {
                    Some(b) => b,
                    None => return Ok(()) // User aborted
                }
            } else {
                current_branch
            };

            // Confirm the pull if in interactive mode
            if interactive {
                let theme = ColorfulTheme::default();
                if !Confirm::with_theme(&theme)
                    .with_prompt(format!("Pull branch '{}' from remote '{}'?", branch_to_pull, remote_name))
                    .default(true)
                    .interact()
                    .unwrap_or(false) {
                    println!("Pull aborted.");
                    return Ok(());
                }
            }

            // Perform the pull
            println!("Pulling branch '{}' from remote '{}'...", branch_to_pull.green().bold(), remote_name.green().bold());

            dot_rev.pull(&remote_name, &branch_to_pull)
                .map_err(|e| match e {
                    DotRevError::RemoteNotFound(name) => AppError::RemoteNotFound(name),
                    DotRevError::BranchNotFound(name) => AppError::BranchNotFound(name),
                    DotRevError::PullError(msg) => AppError::PullError(msg),
                    _ => AppError::DotRevError(e),
                })?;

            println!("Branch '{}' successfully pulled from remote '{}'", branch_to_pull.green().bold(), remote_name.green().bold());
            Ok(())
        },
        Usage { command } => {
            match command {
                Some(cmd) => {
                    match get_command_help(&cmd) {
                        Some(help_text) => println!("{}", help_text),
                        None => println!("No detailed help found for '{}'. Try 'revtool usage' for general help.", cmd),
                    }
                },
                None => println!("{}", show_general_help()),
            }
            Ok(())
        },

        Ignore { pattern, remove } => {
            let (dot_rev, _branch) = get_repository()?;
            let mut ignores = dot_rev.ignores()?;

            match (pattern, remove) {
                // Just list the current ignore patterns
                (None, false) => {
                    println!("{}", "Current ignore patterns:".green().bold());
                    for pattern in &ignores.patterns {
                        println!("  {}", pattern);
                    }
                },
                // Add a new pattern
                (Some(pattern), false) => {
                    // Check if the pattern already exists
                    if ignores.patterns.contains(&pattern) {
                        println!("Pattern '{}' is already in the ignore list", pattern.yellow());
                    } else {
                        ignores.add_pattern(pattern.clone());
                        dot_rev.set_ignores(&ignores)?;
                        println!("Added '{}' to ignore patterns", pattern.green());
                    }
                },
                // Remove a pattern
                (Some(pattern), true) => {
                    if let Some(pos) = ignores.patterns.iter().position(|p| p == &pattern) {
                        ignores.patterns.remove(pos);
                        // Rebuild glob set
                        let new_ignores = Ignores::new(ignores.patterns);
                        dot_rev.set_ignores(&new_ignores)?;
                        println!("Removed '{}' from ignore patterns", pattern.red());
                    } else {
                        println!("Pattern '{}' not found in ignore list", pattern.yellow());
                    }
                },
                // No pattern provided for remove
                (None, true) => {
                    println!("{}", "Error: Must specify a pattern to remove".red().bold());
                    println!("Usage: revtool ignore --remove <pattern>");
                }
            }

            Ok(())
        },
        Reset { delete_absent } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let snapshot_id = dot_rev.branch_snapshot_id(&branch)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let directory: Directory = store.read_json(snapshot.directory)?;

            // Restore files from snapshot
            let cwd = current_dir()?;
            directory.write(&store, &cwd, delete_absent)
                .map_err(|e| AppError::FailedToResetFiles(format!("{:?}", e)))?;

            println!("Reset to the last snapshot on branch '{}' ({})",
                branch.green().bold(),
                snapshot_id.to_string().cyan());
            Ok(())
        }
        Diff { branch } => {
            let (dot_rev, this_branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let that_branch = branch;

            if !dot_rev.branch_exists(&that_branch)? {
                return Err(AppError::BranchNotFound(that_branch));
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
            println!("Diff between branch '{}' and '{}':",
                this_branch.green().bold(),
                that_branch.green().bold());

            // Custom output for diff to add colors
            for line in diff.to_string().lines() {
                if line.starts_with("A ") {
                    println!("{} {}", "A".green().bold(), line[2..].green());
                } else if line.starts_with("D ") {
                    println!("{} {}", "D".red().bold(), line[2..].red());
                } else if line.starts_with("M ") {
                    println!("{} {}", "M".yellow().bold(), line[2..].yellow());
                } else {
                    println!("{}", line);
                }
            }

            Ok(())
        }
        Branch { name } => {
            let (dot_rev, current_branch) = get_repository()?;

            match name {
                Some(branch_name) => {
                    // Create a new branch
                    dot_rev.create_branch(&branch_name)?;
                    println!("Created branch '{}'", branch_name.green().bold());
                    Ok(())
                },
                None => {
                    // List branches and show current
                    let branches = dot_rev.list_branches()?;

                    for branch in branches {
                        if branch == current_branch {
                            println!("* {}", branch.green().bold()); // Current branch marked with asterisk
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
                .map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;
            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;
            let diff = old_directory.diff(&directory);

            println!("On branch {}", branch.green().bold());
            if diff.added.is_empty() && diff.deleted.is_empty() && diff.modified.is_empty() {
                println!("{}", "Working tree clean, nothing to snapshot".green());
            } else {
                println!("\n{}", "Changes not yet snapped:".yellow().bold());
                println!("  (use \"{}\" to create a new snapshot)\n", "revtool snap -m <message>".cyan());

                for file in diff.added.keys() {
                    println!("        {}: {}", "new file".green().bold(), file);
                }

                for file in &diff.deleted {
                    println!("        {}: {}", "deleted".red().bold(), file);
                }

                for file in diff.modified.keys() {
                    println!("        {}: {}", "modified".yellow().bold(), file);
                }
            }
            Ok(())
        },

        Log { limit } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let mut snapshot_id = dot_rev.branch_snapshot_id(&branch)?;

            println!("Commit history for branch '{}':", branch.green().bold());
            println!("{}", "--------------------------------".cyan());

            let mut count = 0;
            loop {
                if count >= limit {
                    break;
                }

                let snapshot: SnapShot = store.read_json(snapshot_id)?;
                println!("{}: {}", "Snapshot".yellow().bold(), snapshot_id.to_string().cyan());
                println!("{}: {}", "Message".yellow().bold(), snapshot.message);
                println!("{}", "--------------------------------".cyan());

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

            // Get the branch to checkout - either from command line or interactively
            let branch_to_checkout = match branch {
                Some(b) => b,
                None if interactive => {
                    match interactive_branch_selection(&dot_rev, &current_branch)? {
                        Some(b) => b,
                        None => return Ok(()) // User aborted
                    }
                },
                None => {
                    return Err(AppError::Other("No branch specified. Please provide a branch name or use interactive mode with -i".to_string()));
                }
            };

            // Don't do anything if trying to checkout the current branch
            if branch_to_checkout == current_branch {
                println!("Already on branch '{}'", branch_to_checkout.green().bold());
                return Ok(());
            }

            // Create the branch if it doesn't exist
            if !dot_rev.branch_exists(&branch_to_checkout)? {
                if interactive {
                    let theme = ColorfulTheme::default();
                    if !Confirm::with_theme(&theme)
                        .with_prompt(format!("Branch '{}' doesn't exist. Create it?", branch_to_checkout))
                        .default(true)
                        .interact()
                        .unwrap_or(false) {
                        println!("Branch creation aborted.");
                        return Ok(());
                    }
                }

                println!("Creating new branch '{}'", branch_to_checkout.green().bold());
                dot_rev.create_branch(&branch_to_checkout)?;
            }

            // Switch to the branch
            dot_rev.set_branch(&branch_to_checkout)?;

            // Reset to the snapshot on this branch
            let snapshot_id = dot_rev.branch_snapshot_id(&branch_to_checkout)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let directory: Directory = store.read_json(snapshot.directory)?;

            // Restore files from snapshot (without deleting files not in snapshot)
            let cwd = current_dir()?;
            directory.write(&store, &cwd, false)
                .map_err(|e| AppError::FailedToRestoreFiles(format!("{:?}", e)))?;

            println!("Switched to branch '{}' (snapshot: {})",
                branch_to_checkout.green().bold(),
                snapshot_id.to_string().cyan());
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
            ).map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;

            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;

            serde_json::to_writer_pretty(stdout(), &old_directory.diff(&directory))
                .map_err(|e| AppError::FailedToOutputChanges(format!("{}", e)))?;
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
            ).map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;

            // Check if there are any changes
            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;
            let diff = old_directory.diff(&directory);

            if diff.added.is_empty() && diff.deleted.is_empty() && diff.modified.is_empty() {
                return Err(AppError::NoChangesToSnapshot);
            }

            // Get the commit message - either from the command line or interactively
            let commit_message = if let Some(msg) = message {
                msg
            } else if interactive {
                match interactive_snapshot_message(&diff)? {
                    Some(msg) => msg,
                    None => return Ok(()) // User aborted
                }
            } else {
                return Err(AppError::Other("No message provided for snapshot. Use -m or --message option, or use interactive mode with -i".to_string()));
            };

            // Store the new directory and create a snapshot
            let directory_id = store.insert_json(&directory)?;
            let snap = SnapShot {
                directory: directory_id,
                previous: vec![old_tip].into_iter().collect(),
                message: commit_message,
            };

            // Store the snapshot and update the branch
            let snap_id = store.insert_json(&snap)?;
            dot_rev.set_branch_snapshot_id(&branch, snap_id)?;

            println!("Created snapshot {} on branch '{}'",
                snap_id.to_string().cyan(),
                branch.green().bold());
            Ok(())
        }
        Init => {
            DotRev::init(current_dir()?.join(".rev"))?;
            println!("{}", "Initialized empty revision control repository in .rev/".green().bold());
            Ok(())
        }
    }
}

fn main() {
    env_logger::init();
    let args = Arguments::parse();

    if let Err(err) = run_command(args.cmd, args.interactive) {
        match err {
            AppError::NoChangesToSnapshot => {
                // This is a common case that isn't actually an error, so don't exit with error code
                println!("{}", "No changes to record in snapshot".yellow());
                println!("Make some changes to files before creating a snapshot.");
            },
            AppError::BranchNotFound(ref branch) => {
                eprintln!("{} {}", "Error:".red().bold(), err);
                println!("To create the branch '{}', use: revtool branch {}", branch, branch);
                exit(1);
            },
            AppError::DotRevError(DotRevError::RepositoryNotInitialized) => {
                eprintln!("{} {}", "Error:".red().bold(), err);
                println!("\nTo initialize a repository in the current directory, run:");
                println!("  revtool init");
                exit(1);
            },
            AppError::RemoteNotFound(ref name) => {
                eprintln!("{} {}", "Error:".red().bold(), err);
                println!("\nTo add a remote named '{}', use:", name);
                println!("  revtool remote add {} <url>", name);
                exit(1);
            },
            AppError::Other(ref msg) if msg.contains("No remote specified") => {
                eprintln!("{} {}", "Error:".red().bold(), err);
                println!("\nTo use with a specific remote, provide the remote name:");
                println!("  revtool push <remote-name> [branch]");
                println!("  revtool pull <remote-name> [branch]");
                println!("\nOr use interactive mode with the -i flag");
                exit(1);
            },
            _ => {
                eprintln!("{} {}", "Error:".red().bold(), err);

                // Provide general help command reminder
                println!("\nFor general help, try:");
                println!("  revtool usage");
                exit(1);
            }
        }
    }
}
