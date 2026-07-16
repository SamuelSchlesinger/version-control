use std::{
    collections::BTreeSet,
    env::current_dir,
    fmt::Debug,
    io::stdout,
    path::PathBuf,
    process::exit
};

use clap::{Parser, Subcommand};
use colored::*;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use lib::{
    directory::{Directory, Ignores},
    dot_rev::{DotRev, Error as DotRevError, InsertJson, MergeState},
    merge::{self, ConflictResolver, MergeResult},
    object_id::ObjectId,
    object_store::ObjectStore,
    snapshot::SnapShot,
    remote::{RemoteConfig, http_client::HttpRemoteClient, http_server::HttpRemoteServer, sync},
};

// Application error type
#[derive(Debug)]
enum AppError {
    DotRevError(DotRevError),
    IoError(std::io::Error),
    #[allow(dead_code)]
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
    MergeConflicts(Vec<merge::MergeConflict>),
    MergeInProgress,
    NoMergeInProgress,
    MergeFailed(String),
    Other(String),
}

impl From<DotRevError> for AppError {
    fn from(err: DotRevError) -> Self {
        match err {
            DotRevError::MergeInProgress => AppError::MergeInProgress,
            DotRevError::NoMergeInProgress => AppError::NoMergeInProgress,
            _ => AppError::DotRevError(err)
        }
    }
}

impl<T> From<merge::Error<T>> for AppError
where
    T: std::fmt::Debug + lib::object_store::ObjectStore,
    T::Error: std::fmt::Debug
{
    fn from(err: merge::Error<T>) -> Self {
        match err {
            merge::Error::UnresolvedConflicts(conflicts) => AppError::MergeConflicts(conflicts),
            _ => AppError::MergeFailed(format!("{:?}", err))
        }
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
                DotRevError::MergeInProgress => write!(f, "A merge is already in progress. Resolve conflicts and use 'revtool merge --continue' or use 'revtool merge --abort' to cancel"),
                DotRevError::NoMergeInProgress => write!(f, "No merge is in progress"),
                DotRevError::InvalidBranchName { name, reason } => write!(
                    f,
                    "Invalid branch name '{name}': {reason}.\n\
                     Branch names must be a single name with no '/', no '..', and no leading '-'."
                ),
                DotRevError::RemoteExists(name) => {
                    write!(f, "A remote named '{name}' already exists. Use a different name or remove it first.")
                }
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
            AppError::MergeConflicts(conflicts) => {
                writeln!(f, "Merge conflicts detected in the following files:")?;
                for conflict in conflicts {
                    writeln!(f, "  - {}: {}", conflict.path.display(), conflict.conflict_type)?;
                }
                write!(f, "Resolve conflicts in these files and run 'revtool merge --continue'")
            },
            AppError::MergeInProgress => write!(f, "A merge is already in progress. Resolve conflicts and use 'revtool merge --continue' or use 'revtool merge --abort' to cancel"),
            AppError::NoMergeInProgress => write!(f, "No merge is in progress"),
            AppError::MergeFailed(reason) => write!(f, "Merge failed: {}. Resolve the issues and try again.", reason),
            AppError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

type AppResult<T> = Result<T, AppError>;


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
        after_help = "Examples:\n  revtool ignore                # List all patterns\n  revtool ignore \"**/*.log\"      # Add a pattern\n  revtool ignore --remove target  # Remove a pattern\n  revtool ignore -i             # Manage patterns interactively"
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
        about = "Check the difference between snapshots",
        long_about = "Compares two snapshots and displays the differences between them",
        after_help = "Examples:\n  revtool diff dev                  # Current branch vs dev branch\n  revtool diff main                 # Current branch vs main branch\n  revtool diff HEAD~1                # Current branch vs its parent\n  revtool diff abc123                # Current branch vs specific snapshot (by ID or prefix)\n  revtool diff main HEAD~2           # Compare main branch with grandparent of current branch\n  revtool diff abc123 def456         # Compare two specific snapshots\n  revtool diff --content main        # Show content-level diffs"
    )]
    Diff {
        #[arg(help = "First snapshot reference (defaults to current branch if omitted when second ref is provided)")]
        first_ref: Option<String>,

        #[arg(help = "Second snapshot reference (defaults to current branch)")]
        second_ref: Option<String>,

        #[arg(long, help = "Show content-level diffs for modified files")]
        content: bool,

        #[arg(long, help = "Number of context lines to show around changes", default_value_t = 3)]
        context: usize,

        #[arg(long, help = "Disable colorized output")]
        no_color: bool
    },

    #[clap(
        about = "Shows files and directories changed since the latest snapshot",
        long_about = "Outputs detailed information about which files have been added, modified, or deleted since the last snapshot",
        after_help = "Example:\n  revtool changes\n  revtool changes --content\n  revtool changes --json"
    )]
    Changes {
        #[arg(long, help = "Show content-level diffs for modified files")]
        content: bool,

        #[arg(long, help = "Output as JSON instead of formatted text")]
        json: bool,

        #[arg(long, help = "Number of context lines to show around changes", default_value_t = 3)]
        context: usize,

        #[arg(long, help = "Disable colorized output")]
        no_color: bool
    },

    #[clap(
        about = "Show working tree status",
        long_about = "Shows which files have been modified, added, or deleted since the last snapshot. Similar to 'git status'",
        after_help = "Example:\n  revtool status\n  revtool status --content"
    )]
    Status {
        #[arg(long, help = "Show content-level diffs for modified files")]
        content: bool,

        #[arg(long, help = "Number of context lines to show around changes", default_value_t = 3)]
        context: usize,

        #[arg(long, help = "Disable colorized output")]
        no_color: bool
    },

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
        about = "Merge changes from another branch into the current branch",
        long_about = "Combines changes from the specified branch with the current branch, creating a new snapshot that includes both sets of changes",
        after_help = "Examples:\n  revtool merge dev                       # Merge dev branch into current branch\n  revtool merge feature -m \"Merge feature\"  # Merge with custom message\n  revtool merge --abort                   # Abort an in-progress merge\n  revtool merge --continue                # Continue a merge after resolving conflicts\n  revtool merge feature --strategy ours    # Auto-resolve conflicts using our version\n  revtool merge feature --editor vim       # Use specific editor for conflict resolution"
    )]
    Merge {
        #[arg(help = "Branch to merge into the current branch")]
        branch: Option<String>,

        #[arg(short, long, help = "Message for the merge snapshot")]
        message: Option<String>,

        #[arg(long, help = "Abort an in-progress merge and restore state")]
        abort: bool,

        #[arg(long, help = "Continue a merge after resolving conflicts")]
        r#continue: bool,

        #[arg(long, help = "Merge strategy: 'normal', 'ours', or 'theirs'")]
        strategy: Option<String>,

        #[arg(long, help = "Specify editor to use for conflict resolution")]
        editor: Option<String>,
    },

    #[clap(
        about = "Switch to a branch",
        long_about = "Changes the current branch to the specified branch and updates the working directory to match",
        after_help = "Example:\n  revtool checkout dev\n  revtool checkout main"
    )]
    Checkout {
        #[arg(help = "Branch to checkout")]
        branch: Option<String>,
        #[arg(
            short = 'b',
            long,
            default_value = "false",
            help = "Create the branch if it does not exist, then switch to it"
        )]
        create: bool,
        #[arg(
            short,
            long,
            default_value = "false",
            help = "Discard uncommitted changes instead of refusing to switch"
        )]
        force: bool,
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
        long_about = "Resets the working directory to match the latest snapshot on the current branch. \
                      This discards uncommitted changes to tracked files, so it requires --force.",
        after_help = "Examples:\n  revtool reset --force              # Restore tracked files, keep untracked files\n  revtool reset --force --delete-absent  # Also delete untracked files"
    )]
    Reset {
        #[arg(
            short,
            long,
            default_value = "false",
            help = "Whether to delete files absent from the snapshot (including untracked files)"
        )]
        delete_absent: bool,
        #[arg(
            short,
            long,
            default_value = "false",
            help = "Required: confirm discarding uncommitted changes"
        )]
        force: bool,
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

    #[clap(
        about = "Manage remote repositories",
        long_about = "Add, remove, or list remote repositories for push/pull operations",
        after_help = "Examples:\n  revtool remote                     # List all remotes\n  revtool remote add origin <url>    # Add a remote named 'origin'\n  revtool remote remove origin       # Remove the 'origin' remote"
    )]
    Remote {
        #[clap(subcommand)]
        cmd: Option<RemoteCommand>,
    },

    #[clap(
        about = "Push changes to a remote repository",
        long_about = "Upload local snapshots and update the remote branch reference",
        after_help = "Examples:\n  revtool push origin                # Push current branch to origin\n  revtool push origin main           # Push main branch to origin\n  revtool push origin main --force   # Force push (overwrite remote)"
    )]
    Push {
        #[arg(help = "Remote repository name")]
        remote: String,

        #[arg(help = "Branch to push (defaults to current branch)")]
        branch: Option<String>,

        #[arg(long, help = "Force push even if not fast-forward")]
        force: bool,
    },

    #[clap(
        about = "Pull changes from a remote repository",
        long_about = "Download snapshots from a remote repository and update the local branch",
        after_help = "Examples:\n  revtool pull origin                # Pull current branch from origin\n  revtool pull origin main           # Pull main branch from origin"
    )]
    Pull {
        #[arg(help = "Remote repository name")]
        remote: String,

        #[arg(help = "Branch to pull (defaults to current branch)")]
        branch: Option<String>,
    },

    #[clap(
        about = "Start an HTTP server to host the repository",
        long_about = "Run an HTTP server that allows other users to push/pull from this repository",
        after_help = "Example:\n  revtool serve                      # Serve on default port 8080\n  revtool serve --port 3000          # Serve on port 3000"
    )]
    Serve {
        #[arg(long, default_value = "8080", help = "Port to listen on")]
        port: u16,

        #[arg(long, default_value = "127.0.0.1", help = "Address to bind to")]
        host: String,

        #[arg(
            long,
            help = "Require this bearer token on every request (or set REVTOOL_TOKEN)"
        )]
        token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum RemoteCommand {
    #[clap(about = "Add a new remote repository")]
    Add {
        #[arg(help = "Name for the remote")]
        name: String,

        #[arg(help = "URL of the remote repository")]
        url: String,
    },

    #[clap(about = "Remove a remote repository")]
    Remove {
        #[arg(help = "Name of the remote to remove")]
        name: String,
    },
}

// A helper function to get the repository
fn get_repository() -> AppResult<(DotRev, String)> {
    let dot_rev = DotRev::here()?;
    let branch = dot_rev.branch()?;
    Ok((dot_rev, branch))
}

/// Returns the paths that differ between the working tree and the current
/// branch's snapshot: files added, modified, or deleted but not yet snapped.
///
/// This is the "dirty set" used to protect uncommitted work before an operation
/// that would overwrite it (checkout, reset). An empty result means the working
/// tree matches the snapshot and is safe to replace.
fn uncommitted_changes(
    dot_rev: &DotRev,
    store: &mut lib::object_store::directory::DirectoryObjectStore,
) -> AppResult<Vec<PathBuf>> {
    let ignores = dot_rev.ignores()?;
    let cwd = dot_rev.work_dir().to_path_buf();
    let working = Directory::new(cwd.as_path(), &ignores, store)
        .map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;

    let tip = dot_rev.current_snapshot_id()?;
    let snapshot: SnapShot = store.read_json(tip)?;
    let committed: Directory = store.read_json(snapshot.directory)?;

    let diff = committed.diff(&working);
    let mut paths: Vec<PathBuf> = Vec::new();
    paths.extend(diff.added.keys().map(PathBuf::from));
    paths.extend(diff.deleted.iter().map(PathBuf::from));
    paths.extend(diff.modified.keys().map(PathBuf::from));
    paths.sort();
    Ok(paths)
}

/// Refuses the operation if the working tree has uncommitted changes, unless
/// `force` is set. Prints the offending paths and how to proceed.
fn ensure_clean_or_forced(
    dot_rev: &DotRev,
    store: &mut lib::object_store::directory::DirectoryObjectStore,
    force: bool,
    action: &str,
) -> AppResult<()> {
    if force {
        return Ok(());
    }
    let dirty = uncommitted_changes(dot_rev, store)?;
    if dirty.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "You have uncommitted changes that {action} would overwrite:\n"
    );
    for path in &dirty {
        msg.push_str(&format!("  {}\n", path.display()));
    }
    msg.push_str(
        "\nSnapshot them with 'revtool snap -m <message>' first, or re-run with --force to discard them",
    );
    Err(AppError::Other(msg))
}

/// True if any line of `content` is a conflict marker, i.e. begins with a run
/// of seven or more of `<`, `=`, `>`, or `|`. Anchoring to the start of a line
/// avoids false positives on prose like a row of `====` in a comment.
fn content_has_conflict_markers(content: &[u8]) -> bool {
    content.split(|&b| b == b'\n').any(|line| {
        b"<=>|".iter().any(|&marker| {
            line.iter().take_while(|&&b| b == marker).count() >= 7
        })
    })
}

/// Rewrites the working tree to match a snapshot's directory. Used after a
/// merge finalizes so that conflict-marker files left on disk are replaced with
/// the resolved content that was actually committed. Without this, the working
/// tree and the snapshot disagree and a following `snap` would re-commit the
/// markers.
fn refresh_worktree_to_snapshot(
    dot_rev: &DotRev,
    store: &mut lib::object_store::directory::DirectoryObjectStore,
    snapshot_id: ObjectId,
) -> AppResult<()> {
    let snapshot: SnapShot = store.read_json(snapshot_id)?;
    let directory: Directory = store.read_json(snapshot.directory)?;
    let cwd = dot_rev.work_dir().to_path_buf();
    directory
        .write(&*store, &cwd, false)
        .map_err(|e| AppError::FailedToRestoreFiles(format!("{:?}", e)))?;
    Ok(())
}

/// Masks the password in a `scheme://user:pass@host` URL so it is not printed
/// to the terminal. Remotes are stored with whatever credentials the user
/// embedded in the URL; displaying them verbatim leaks secrets to anyone
/// watching the screen or reading logs.
fn redact_url_credentials(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let after = scheme_end + 3;
        let rest = &url[after..];
        let host_start = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..host_start].find('@') {
            let userinfo = &rest[..at];
            if let Some(colon) = userinfo.find(':') {
                return format!("{}{}:***@{}", &url[..after], &userinfo[..colon], &rest[at + 1..]);
            }
        }
    }
    url.to_string()
}

/// Removes working-tree files that were tracked on `current` but are absent
/// from `target`, so switching branches does not leave the old branch's files
/// polluting the new branch. Untracked files (in neither snapshot) are left
/// alone; this only touches paths the old snapshot tracked.
fn remove_tracked_absent(
    current: &Directory,
    target: &Directory,
    root: &std::path::Path,
) -> AppResult<()> {
    let target_paths: BTreeSet<PathBuf> =
        target.files().into_iter().map(|(p, _)| p).collect();
    for (path, _) in current.files() {
        if !target_paths.contains(&path) {
            let full = root.join(&path);
            if full.exists() {
                std::fs::remove_file(&full).map_err(AppError::IoError)?;
            }
        }
    }
    Ok(())
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

    // Use the DiffFormatter for consistent display
    use lib::diff_format::{DiffFormatter, FormatOptions};

    // Configure formatter options - preserve color but hide content diffs for the snapshot preview
    // We'll keep color on for the snapshot preview by default, since it's part of the interactive UI
    let format_options = FormatOptions {
        use_color: true,
        show_content: false,  // Don't show content diffs in the snapshot preview
        context_lines: 3,     // Standard context lines
        show_stats: true,     // Show stats summary
    };

    let formatter = DiffFormatter::new(diff, format_options);
    println!("{}", formatter);

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

/// Interactive mode helper for resolving merge conflicts
fn interactive_merge_conflict_resolution<S>(
    dot_rev: &DotRev,
    conflicts: &[merge::MergeConflict],
    store: &mut S,
    editor_override: Option<&str>,
    strategy: Option<merge::MergeStrategy>
) -> AppResult<MergeResult>
where
    S: InsertJson + ObjectStore + std::fmt::Debug,
    S::Error: std::fmt::Debug,
{
    let theme = ColorfulTheme::default();

    println!("\n{}", "Merge conflicts detected:".yellow().bold());
    for (i, conflict) in conflicts.iter().enumerate() {
        println!("  {}: {} ({})", i + 1, conflict.path.display(), conflict.conflict_type);
    }

    // Get the merge state
    let mut merge_state = dot_rev.get_merge_state()?;

    // Check if we have a strategy to automatically resolve conflicts
    if let Some(strategy) = strategy {
        match strategy {
            merge::MergeStrategy::Normal => {
                // Normal strategy - continue with manual resolution
                println!("Using normal merge strategy - conflicts will be resolved manually.");
            },
            merge::MergeStrategy::Ours | merge::MergeStrategy::Theirs => {
                println!("Applying {} merge strategy to all conflicts...",
                    if strategy == merge::MergeStrategy::Ours { "ours".cyan() } else { "theirs".cyan() });

                // Track success or failure for each conflict
                let mut all_resolved = true;
                let mut failed_paths = Vec::new();

                // Process each conflict with the given strategy
                for conflict in conflicts {
                    let resolver = ConflictResolver::new(store, conflict.clone(), &merge_state.current_branch, &merge_state.merge_branch)?;

                    // Try to apply the strategy
                    match resolver.resolve_with_strategy(store, strategy)? {
                        Some(resolved_id) => {
                            // Update the conflict in the merge result
                            for c in merge_state.merge_result.conflicts.iter_mut() {
                                if c.path == conflict.path {
                                    c.resolved = true;
                                    c.resolution_id = Some(resolved_id);
                                    break;
                                }
                            }

                            println!("Resolved: {} ({})", conflict.path.display(), conflict.conflict_type);
                        },
                        None => {
                            // Strategy couldn't be applied
                            all_resolved = false;
                            failed_paths.push(conflict.path.display().to_string());
                        }
                    }
                }

                // If all conflicts were resolved, return early
                if all_resolved {
                    println!("\n{}", "All conflicts resolved automatically!".green().bold());

                    // Update the merge state
                    dot_rev.save_merge_state(&merge_state)?;

                    return Ok(merge_state.merge_result);
                } else {
                    println!("\n{}", "Some conflicts could not be resolved automatically:".yellow().bold());
                    for path in &failed_paths {
                        println!("  - {}", path);
                    }
                    println!("These conflicts will need to be resolved manually.");
                }
            }
        }
    }

    // Create a mutable copy of conflicts for tracking resolution
    let mut pending_conflicts = conflicts.iter()
        .filter(|c| !c.resolved)
        .cloned()
        .collect::<Vec<_>>();

    // Process each conflict
    while !pending_conflicts.is_empty() {
        println!("\n{}", "Unresolved conflicts:".cyan().bold());
        for (i, conflict) in pending_conflicts.iter().enumerate() {
            println!("  {}: {} ({})", i + 1, conflict.path.display(), conflict.conflict_type);
        }

        // Choose a conflict to resolve
        let selection = Select::with_theme(&theme)
            .with_prompt("Select conflict to resolve")
            .default(0)
            .items(&pending_conflicts.iter().map(|c| c.path.display().to_string()).collect::<Vec<_>>())
            .interact()
            .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

        let conflict = pending_conflicts[selection].clone();

        // Create a resolver for this conflict
        let resolver = ConflictResolver::new(store, conflict.clone(), &merge_state.current_branch, &merge_state.merge_branch)?;

        // Show conflict details
        println!("\n{}", "Conflict details:".green().bold());
        println!("  Path: {}", conflict.path.display());
        println!("  Type: {}", conflict.conflict_type);

        // Show options for resolution
        println!("\n{}", "How would you like to resolve this conflict?".cyan().bold());
        let options = vec!["Use our version", "Use their version", "Edit manually", "Skip for now"];

        let resolution = Select::with_theme(&theme)
            .with_prompt("Resolution strategy")
            .default(0)
            .items(&options)
            .interact()
            .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

        match resolution {
            0 => {
                // Use our version
                if let Some(ours_id) = conflict.ours_id {
                    // Get the content
                    let content = store.read(ours_id).map_err(|_| AppError::MissingObject(ours_id))?
                        .ok_or(AppError::MissingObject(ours_id))?;

                    // Save and mark as resolved
                    let resolved_id = resolver.resolve(store, &content)?;

                    // Update the conflict in the merge result
                    for c in merge_state.merge_result.conflicts.iter_mut() {
                        if c.path == conflict.path {
                            c.resolved = true;
                            c.resolution_id = Some(resolved_id);
                            break;
                        }
                    }

                    // Remove from pending conflicts
                    pending_conflicts.remove(selection);

                    println!("Conflict resolved: Used our version for {}", conflict.path.display());
                } else {
                    println!("{}", "No 'our' version available for this conflict".red());
                }
            },
            1 => {
                // Use their version
                if let Some(theirs_id) = conflict.theirs_id {
                    // Get the content
                    let content = store.read(theirs_id).map_err(|_| AppError::MissingObject(theirs_id))?
                        .ok_or(AppError::MissingObject(theirs_id))?;

                    // Save and mark as resolved
                    let resolved_id = resolver.resolve(store, &content)?;

                    // Update the conflict in the merge result
                    for c in merge_state.merge_result.conflicts.iter_mut() {
                        if c.path == conflict.path {
                            c.resolved = true;
                            c.resolution_id = Some(resolved_id);
                            break;
                        }
                    }

                    // Remove from pending conflicts
                    pending_conflicts.remove(selection);

                    println!("Conflict resolved: Used their version for {}", conflict.path.display());
                } else {
                    println!("{}", "No 'their' version available for this conflict".red());
                }
            },
            2 => {
                // Manual edit - show the conflict markers
                println!("\n{}", "Manual conflict resolution:".yellow().bold());
                println!("The file with conflict markers has been written to disk.");
                println!("Please edit the file to resolve the conflict, removing the conflict markers.");
                println!("Conflict markers look like this:");
                println!("<<<<<<< HEAD (Current branch: {})", merge_state.current_branch);
                println!("Our content");
                println!("=======");
                println!("Their content");
                println!(">>>>>>> {} (Incoming changes)", merge_state.merge_branch);
                println!("||||||| BASE (common ancestor)");
                println!("Base content");
                println!("# Additional instructions are included in the file");

                // Write the marked content to the file
                let file_path = conflict.path.clone();
                std::fs::write(&file_path, &resolver.marked_content)
                    .map_err(|e| AppError::Other(format!("Failed to write conflict file: {}", e)))?;

                // Launch editor if specified
                if let Some(editor) = editor_override {
                    // Attempt to launch the specified editor
                    println!("Launching editor: {}", editor);

                    use std::process::Command;
                    // Split on whitespace so editors with arguments work, e.g.
                    // --editor "code -w" or --editor "emacsclient -nw". Command
                    // does no shell parsing, so the flags must be separate args.
                    let mut parts = editor.split_whitespace();
                    let program = parts.next().ok_or_else(|| {
                        AppError::Other("empty --editor value".to_string())
                    })?;
                    let status = Command::new(program)
                        .args(parts)
                        .arg(&file_path)
                        .status()
                        .map_err(|e| AppError::Other(format!("Failed to launch editor {}: {}", editor, e)))?;

                    if !status.success() {
                        println!("{}", format!("Editor exited with non-zero status: {}", status).red());
                    }
                }

                // Prompt to continue after editing
                if Confirm::with_theme(&theme)
                    .with_prompt(format!("Have you finished editing {}?", file_path.display()))
                    .default(false)
                    .interact()
                    .unwrap_or(false)
                {
                    // Read back the edited file
                    let resolved_content = std::fs::read(&file_path)
                        .map_err(|e| AppError::Other(format!("Failed to read edited file: {}", e)))?;

                    // Check if conflict markers are still present
                    let content_str = String::from_utf8_lossy(&resolved_content);
                    if content_str.contains("<<<<<<<") || content_str.contains(">>>>>>>") ||
                       content_str.contains("=======") || content_str.contains("|||||||") ||
                       content_str.contains("# CONFLICT RESOLUTION INSTRUCTIONS") {
                        println!("{}", "Conflict markers are still present in the file. Please remove all marker lines including:".red());
                        println!("  - <<<<<<< HEAD (or similar)");
                        println!("  - =======");
                        println!("  - >>>>>>> (branch name)");
                        println!("  - ||||||| BASE");
                        println!("  - All lines starting with #");
                        continue;
                    }

                    // Save and mark as resolved
                    let resolved_id = resolver.resolve(store, &resolved_content)?;

                    // Update the conflict in the merge result
                    for c in merge_state.merge_result.conflicts.iter_mut() {
                        if c.path == conflict.path {
                            c.resolved = true;
                            c.resolution_id = Some(resolved_id);
                            break;
                        }
                    }

                    // Remove from pending conflicts
                    pending_conflicts.remove(selection);

                    println!("Conflict resolved: Manual edit for {}", conflict.path.display());
                }
            },
            3 => {
                // Skip for now
                println!("Skipping conflict for {}", conflict.path.display());
            },
            _ => unreachable!(),
        }
    }

    // Check if all conflicts are resolved
    let all_resolved = merge_state.merge_result.conflicts.iter().all(|c| c.resolved);

    if all_resolved {
        println!("\n{}", "All conflicts resolved!".green().bold());

        // Update the merge state
        dot_rev.save_merge_state(&merge_state)?;
    } else {
        println!("\n{}", "Some conflicts are still unresolved.".yellow().bold());

        // Update the merge state
        dot_rev.save_merge_state(&merge_state)?;

        return Err(AppError::MergeConflicts(merge_state.merge_result.conflicts.clone()));
    }

    Ok(merge_state.merge_result)
}

/// Interactive mode helper for managing ignore patterns
fn interactive_ignore_management(dot_rev: &DotRev, mut ignores: Ignores) -> AppResult<()> {
    let theme = ColorfulTheme::default();

    loop {
        // Show current patterns
        println!("\n{}", "Current ignore patterns:".green().bold());
        for (i, pattern) in ignores.patterns.iter().enumerate() {
            println!("  {}: {}", i + 1, pattern);
        }

        // Show options
        println!("\n{}", "What would you like to do?".cyan().bold());
        let options = vec!["Add a pattern", "Remove a pattern", "Done"];

        let selection = Select::with_theme(&theme)
            .with_prompt("Select action")
            .default(0)
            .items(&options)
            .interact()
            .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

        match selection {
            0 => {
                // Add a pattern
                let new_pattern: String = Input::with_theme(&theme)
                    .with_prompt("Enter pattern to ignore")
                    .validate_with(|input: &String| -> Result<(), &str> {
                        if input.trim().is_empty() {
                            Err("Pattern cannot be empty")
                        } else {
                            Ok(())
                        }
                    })
                    .interact_text()
                    .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

                if ignores.patterns.contains(&new_pattern) {
                    println!("Pattern '{}' is already in the ignore list", new_pattern.yellow());
                } else {
                    ignores.add_pattern(new_pattern.clone());
                    dot_rev.set_ignores(&ignores)?;
                    println!("Added '{}' to ignore patterns", new_pattern.green());
                }
            },
            1 => {
                // Remove a pattern
                if ignores.patterns.is_empty() {
                    println!("{}", "No patterns to remove".yellow());
                    continue;
                }

                let options: Vec<&String> = ignores.patterns.iter().collect();
                let selection = Select::with_theme(&theme)
                    .with_prompt("Select pattern to remove")
                    .default(0)
                    .items(&options)
                    .interact()
                    .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

                let pattern = ignores.patterns.remove(selection);
                let new_ignores = Ignores::new(ignores.patterns.clone());
                dot_rev.set_ignores(&new_ignores)?;
                ignores = new_ignores;

                println!("Removed '{}' from ignore patterns", pattern.red());
            },
            2 => {
                // Exit the loop
                break;
            },
            _ => unreachable!(),
        }
    }

    Ok(())
}

// Define help texts for command examples
pub fn get_command_help(command: &str) -> Option<String> {
    match command.to_lowercase().as_str() {
        "ignore" => Some(r#"
Manage files and directories to ignore:
  revtool ignore                # List all current ignore patterns
  revtool ignore <pattern>      # Add a new pattern to ignore
  revtool ignore --remove <pattern>  # Remove a pattern from ignore list
  revtool ignore -i             # Interactive pattern management

Examples:
  revtool ignore "**/*.log"    # Ignore all .log files in any directory
  revtool ignore "build/"      # Ignore the build directory
  revtool ignore "**/*.tmp"    # Ignore all .tmp files
  revtool ignore -i            # Manage patterns interactively

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
        "merge" => Some(r#"
Merge changes from another branch into the current branch:
  revtool merge <branch>               # Merge <branch> into current branch
  revtool merge <branch> -m "message"  # Merge with custom message

The merge operation combines changes from the specified branch with your current branch.
This creates a merge snapshot with two parent snapshots.

When conflicts occur:
1. Files with conflicts will be marked with conflict markers
2. Resolve the conflicts by editing these files
3. Run 'revtool merge --continue' to complete the merge

Example workflow:
  revtool branch feature     # Create a feature branch
  revtool checkout feature   # Switch to it
  # Make changes...
  revtool snap -m "Add X"    # Commit changes on feature branch
  revtool checkout main      # Switch back to main branch
  revtool merge feature      # Merge feature branch into main

Options:
  --continue                 # Continue a merge after resolving conflicts
  --abort                    # Abort an in-progress merge and restore state
"#.to_string()),
        "diff" => Some(r#"
Compare snapshots with flexible referencing:
  revtool diff <ref>                     # Compare current branch with <ref>
  revtool diff <ref1> <ref2>             # Compare <ref1> with <ref2>
  revtool diff --content <ref>           # Show content-level diffs

Snapshot reference syntax:
  branch_name       # Latest snapshot on a branch (e.g., "main", "dev")
  HEAD              # Latest snapshot on current branch
  HEAD~N            # N snapshots back from HEAD (e.g., "HEAD~1" for parent)
  branch_name~N     # N snapshots back from branch tip (e.g., "main~2")
  abc123            # Snapshot ID (prefix or full hash)
  abc123~N          # N snapshots back from specific snapshot

Examples:
  revtool diff main                # Compare current branch with main
  revtool diff HEAD~1              # Compare current branch with its parent
  revtool diff a1f2305             # Compare current branch with specific snapshot (using ID prefix)
  revtool diff main dev            # Compare main branch with dev branch
  revtool diff main~1 dev~2        # Compare main's parent with dev's grandparent
  revtool diff --content HEAD~1    # Show content-level diffs with parent
  revtool diff --context 5 HEAD~1  # Show diffs with 5 lines of context
  revtool diff --no-color main     # Show without colors

With --content, shows line-by-line differences within modified files."#.to_string()),
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
        "remote" => Some(r#"
Manage remote repositories:
  revtool remote                    # List all configured remotes
  revtool remote add <name> <url>   # Add a new remote
  revtool remote remove <name>      # Remove a remote

Examples:
  revtool remote                    # Shows all remotes
  revtool remote add origin http://example.com:8080  # Add 'origin' remote
  revtool remote remove backup      # Remove 'backup' remote

Remote repositories allow you to share your snapshots with others."#.to_string()),
        "push" => Some(r#"
Push changes to a remote repository:
  revtool push <remote>             # Push current branch to remote
  revtool push <remote> <branch>    # Push specific branch to remote
  revtool push <remote> --force     # Force push (overwrites remote)

Examples:
  revtool push origin               # Push current branch to origin
  revtool push origin main          # Push main branch to origin
  revtool push origin dev --force   # Force push dev branch

Push uploads your local snapshots to the remote repository."#.to_string()),
        "pull" => Some(r#"
Pull changes from a remote repository:
  revtool pull <remote>             # Pull current branch from remote
  revtool pull <remote> <branch>    # Pull specific branch from remote

Examples:
  revtool pull origin               # Pull current branch from origin
  revtool pull origin main          # Pull main branch from origin

Pull downloads snapshots from the remote and updates your local branch."#.to_string()),
        "serve" => Some(r#"
Start an HTTP server to host the repository:
  revtool serve                     # Serve on default port 8080
  revtool serve --port <port>       # Serve on specific port
  revtool serve --host <host>       # Bind to specific address

Examples:
  revtool serve                     # Start server on 127.0.0.1:8080
  revtool serve --port 3000         # Start server on port 3000
  revtool serve --host 0.0.0.0     # Listen on all interfaces

The server allows other users to push/pull from your repository."#.to_string()),
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

5. Working with remotes:
   revtool remote add origin http://server:8080  # Add a remote
   revtool push origin main      # Push changes
   revtool pull origin main      # Pull changes

Use 'revtool usage <command>' for detailed help on a specific command.
"###.to_string()
}

fn run_command(cmd: Command, interactive: bool) -> AppResult<()> {
    use Command::*;

    // Special cases that don't require an initialized repository
    match &cmd {
        // These commands should work without a repository
        Usage { .. } => {},
        Init => {},
        Serve { .. } => {}, // Serve can work with just a path to a repository
        // All other commands require a repository
        _ => {
            // Only check for repository if we're not initializing or showing usage
            if let Err(e) = DotRev::here() {
                return Err(AppError::DotRevError(e));
            }
        }
    }

    match cmd {
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

        Merge { branch, message, abort, r#continue, strategy, editor } => {
            let (dot_rev, current_branch) = get_repository()?;
            let mut store = dot_rev.store()?;

            // Handle abort case
            if abort {
                // Check if there's a merge in progress
                if !dot_rev.is_merge_in_progress()? {
                    return Err(AppError::NoMergeInProgress);
                }

                // Get the merge state
                let merge_state = dot_rev.get_merge_state()?;

                // Reset to the backup snapshot
                let snapshot_id = merge_state.backup_snapshot_id;
                let snapshot: SnapShot = store.read_json(snapshot_id)?;
                let directory: Directory = store.read_json(snapshot.directory)?;

                // Reset files
                let cwd = dot_rev.work_dir().to_path_buf();
                directory.write(&store, &cwd, false)
                    .map_err(|e| AppError::FailedToResetFiles(format!("{:?}", e)))?;

                // Restore the branch pointer too, so the tip and the working
                // tree agree again even if a snapshot was taken mid-merge.
                dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;

                // Clear the merge state
                dot_rev.clear_merge_state()?;

                println!("Merge aborted. Files and branch reset to their state before the merge.");
                return Ok(());
            }

            // Handle continue case
            if r#continue {
                // Check if there's a merge in progress
                if !dot_rev.is_merge_in_progress()? {
                    return Err(AppError::NoMergeInProgress);
                }

                // Get the merge state
                let mut merge_state = dot_rev.get_merge_state()?;

                // Check if there are unresolved conflicts
                if merge_state.merge_result.has_unresolved_conflicts() {
                    // Process strategy option if provided
                    let merge_strategy = if let Some(strategy_str) = &strategy {
                        match merge::MergeStrategy::from_str(strategy_str) {
                            // 'normal' means manual resolution; treat as no strategy.
                            Ok(merge::MergeStrategy::Normal) => None,
                            Ok(s) => Some(s),
                            Err(e) => {
                                return Err(AppError::Other(e));
                            }
                        }
                    } else {
                        None
                    };

                    // In interactive mode, help resolve conflicts
                    if interactive {
                        let merge_result = interactive_merge_conflict_resolution(
                            &dot_rev,
                            &merge_state.merge_result.conflicts,
                            &mut store,
                            editor.as_deref(),
                            merge_strategy
                        )?;

                        // Create a merge snapshot
                        let merge_msg = message.unwrap_or_else(||
                            format!("Merge branch '{}' into {}",
                                merge_state.merge_branch,
                                merge_state.current_branch
                            )
                        );

                        let snapshot_id = merge_result.create_snapshot(&mut store, merge_msg)?;

                        // Update the branch pointer
                        dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;
                        refresh_worktree_to_snapshot(&dot_rev, &mut store, snapshot_id)?;

                        // Clear the merge state
                        dot_rev.clear_merge_state()?;

                        println!("Merge completed successfully: created snapshot {}",
                            snapshot_id.to_string().cyan());
                        return Ok(());
                    } else if let Some(strategy) = merge_strategy {
                        // In non-interactive mode but with a strategy, we can try to resolve conflicts automatically
                        if strategy == merge::MergeStrategy::Normal {
                            // Normal strategy needs interactive mode
                            return Err(AppError::Other(
                                "Normal merge strategy requires interactive mode. Use 'revtool merge -i'".to_string()
                            ));
                        }

                        println!("Applying {} merge strategy to all conflicts...",
                            if strategy == merge::MergeStrategy::Ours { "ours".cyan() } else { "theirs".cyan() });

                        // Process each conflict with the given strategy
                        let mut all_resolved = true;

                        // Clone conflicts for iteration
                        let conflicts_to_process: Vec<_> = merge_state.merge_result.conflicts.iter()
                            .filter(|c| !c.resolved)
                            .cloned()
                            .collect();

                        for conflict in conflicts_to_process {
                            let resolver = ConflictResolver::new(&store, conflict.clone(), &merge_state.current_branch, &merge_state.merge_branch)?;

                            // Try to apply the strategy
                            match resolver.resolve_with_strategy(&mut store, strategy)? {
                                Some(resolved_id) => {
                                    // Update the conflict in the merge result
                                    for c in merge_state.merge_result.conflicts.iter_mut() {
                                        if c.path == conflict.path {
                                            c.resolved = true;
                                            c.resolution_id = Some(resolved_id);
                                            break;
                                        }
                                    }

                                    println!("Resolved: {} ({})", conflict.path.display(), conflict.conflict_type);
                                },
                                None => {
                                    // Strategy couldn't be applied
                                    all_resolved = false;
                                    println!("Could not resolve: {} ({})", conflict.path.display(), conflict.conflict_type);
                                }
                            }
                        }

                        // Check if all conflicts are now resolved
                        if all_resolved || merge_state.merge_result.conflicts.iter().all(|c| c.resolved) {
                            println!("\n{}", "All conflicts resolved automatically!".green().bold());

                            // Create the merge snapshot
                            let merge_msg = message.unwrap_or_else(||
                                format!("Merge branch '{}' into {} (strategy: {})",
                                    merge_state.merge_branch,
                                    merge_state.current_branch,
                                    if strategy == merge::MergeStrategy::Ours { "ours" } else { "theirs" }
                                )
                            );

                            let snapshot_id = merge_state.merge_result.create_snapshot(&mut store, merge_msg)?;

                            // Update the branch pointer
                            dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;
                            refresh_worktree_to_snapshot(&dot_rev, &mut store, snapshot_id)?;

                            // Clear the merge state
                            dot_rev.clear_merge_state()?;

                            println!("Merge completed successfully: created snapshot {}",
                                snapshot_id.to_string().cyan());
                            return Ok(());
                        } else {
                            // Not all conflicts could be resolved automatically
                            dot_rev.save_merge_state(&merge_state)?;
                            return Err(AppError::MergeConflicts(merge_state.merge_result.conflicts.clone()));
                        }
                    } else {
                        // Non-interactive continue with no strategy: pick up the
                        // resolutions the user made by editing the working-tree
                        // files, the standard git-style workflow.
                        let work_dir = dot_rev.work_dir().to_path_buf();
                        let mut still_conflicted: Vec<PathBuf> = Vec::new();

                        let unresolved: Vec<_> = merge_state
                            .merge_result
                            .conflicts
                            .iter()
                            .filter(|c| !c.resolved)
                            .map(|c| c.path.clone())
                            .collect();

                        for path in unresolved {
                            let full = work_dir.join(&path);
                            let content = match std::fs::read(&full) {
                                Ok(c) => c,
                                Err(_) => {
                                    still_conflicted.push(path);
                                    continue;
                                }
                            };
                            if content_has_conflict_markers(&content) {
                                still_conflicted.push(path);
                                continue;
                            }
                            let resolved_id = store.insert(&content)
                                .map_err(|e| AppError::Other(format!("Failed to store resolved file: {:?}", e)))?;
                            for c in merge_state.merge_result.conflicts.iter_mut() {
                                if c.path == path {
                                    c.resolved = true;
                                    c.resolution_id = Some(resolved_id);
                                    break;
                                }
                            }
                        }

                        if !still_conflicted.is_empty() {
                            // No need to persist progress: the working-tree edits
                            // themselves are the progress, and the next
                            // --continue re-reads them.
                            let mut msg = String::from(
                                "Some files still contain conflict markers:\n",
                            );
                            for path in &still_conflicted {
                                msg.push_str(&format!("  {}\n", path.display()));
                            }
                            msg.push_str("Edit them to remove the markers, then run 'revtool merge --continue'");
                            return Err(AppError::Other(msg));
                        }
                        // All conflicts resolved from the working tree; fall
                        // through to finalization below.
                    }
                }

                // All conflicts are resolved, create the merge snapshot
                let merge_msg = message.unwrap_or_else(||
                    format!("Merge branch '{}' into {}",
                        merge_state.merge_branch,
                        merge_state.current_branch
                    )
                );

                let snapshot_id = merge_state.merge_result.create_snapshot(&mut store, merge_msg)?;

                // Update the branch pointer
                dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;
                refresh_worktree_to_snapshot(&dot_rev, &mut store, snapshot_id)?;

                // Clear the merge state
                dot_rev.clear_merge_state()?;

                println!("Merge completed successfully: created snapshot {}",
                    snapshot_id.to_string().cyan());
                return Ok(());
            }

            // Handle new merge case
            if dot_rev.is_merge_in_progress()? {
                return Err(AppError::MergeInProgress);
            }

            // Make sure a branch is specified
            let merge_branch = match branch {
                Some(b) => b,
                None if interactive => {
                    // In interactive mode, let the user select a branch
                    let branches = dot_rev.list_branches()?;
                    let filtered_branches: Vec<String> = branches
                        .into_iter()
                        .filter(|b| b != &current_branch)
                        .collect();

                    if filtered_branches.is_empty() {
                        return Err(AppError::Other("No other branches found to merge".to_string()));
                    }

                    let theme = ColorfulTheme::default();
                    let selection = Select::with_theme(&theme)
                        .with_prompt("Select branch to merge")
                        .items(&filtered_branches)
                        .default(0)
                        .interact()
                        .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

                    filtered_branches[selection].clone()
                },
                None => {
                    return Err(AppError::Other("No branch specified for merge. Use 'revtool merge <branch>' to specify a branch.".to_string()));
                }
            };

            // Check if the branch exists
            if !dot_rev.branch_exists(&merge_branch)? {
                return Err(AppError::BranchNotFound(merge_branch));
            }

            // Get the snapshot IDs
            let ours_id = dot_rev.branch_snapshot_id(&current_branch)?;
            let theirs_id = dot_rev.branch_snapshot_id(&merge_branch)?;

            // Find common ancestor
            let base_id = match merge::find_common_ancestor(&store, ours_id, theirs_id)? {
                Some(id) => id,
                None => return Err(AppError::Other("No common ancestor found between branches".to_string())),
            };

            // Perform the merge
            let merge_result = merge::merge(&mut store, base_id, ours_id, theirs_id, &current_branch, &merge_branch, true)?;

            // If there are conflicts, handle them
            if !merge_result.success {
                println!("{}", "Merge resulted in conflicts".yellow().bold());

                // Save merge state for later continuation
                let merge_state = MergeState {
                    current_branch: current_branch.clone(),
                    merge_branch: merge_branch.clone(),
                    merge_result: merge_result.clone(),
                    backup_snapshot_id: ours_id,
                };

                dot_rev.save_merge_state(&merge_state)?;

                // Process strategy option if provided
                let merge_strategy = if let Some(strategy_str) = &strategy {
                    match merge::MergeStrategy::from_str(strategy_str) {
                        // 'normal' means "no auto-resolution": fall through to
                        // the manual, marker-based path, exactly as if no
                        // strategy were given.
                        Ok(merge::MergeStrategy::Normal) => None,
                        Ok(s) => Some(s),
                        Err(e) => {
                            return Err(AppError::Other(e));
                        }
                    }
                } else {
                    None
                };

                // In interactive mode, help resolve conflicts
                if interactive {
                    let merge_result = interactive_merge_conflict_resolution(
                        &dot_rev,
                        &merge_result.conflicts,
                        &mut store,
                        editor.as_deref(),
                        merge_strategy
                    )?;

                    // Create a merge snapshot
                    let merge_msg = message.unwrap_or_else(||
                        format!("Merge branch '{}' into {}", merge_branch, current_branch)
                    );

                    let snapshot_id = merge_result.create_snapshot(&mut store, merge_msg)?;

                    // Update the branch pointer
                    dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;
                    refresh_worktree_to_snapshot(&dot_rev, &mut store, snapshot_id)?;

                    // Clear the merge state
                    dot_rev.clear_merge_state()?;

                    println!("Merge completed successfully: created snapshot {}",
                        snapshot_id.to_string().cyan());
                    return Ok(());
                } else if let Some(strategy) = merge_strategy {
                    // In non-interactive mode but with a strategy, we can try to resolve conflicts automatically
                    if strategy == merge::MergeStrategy::Normal {
                        // Normal strategy needs interactive mode
                        return Err(AppError::Other(
                            "Normal merge strategy requires interactive mode. Use 'revtool merge -i'".to_string()
                        ));
                    }

                    println!("Applying {} merge strategy to all conflicts...",
                        if strategy == merge::MergeStrategy::Ours { "ours".cyan() } else { "theirs".cyan() });

                    // Get the merge state
                    let mut merge_state = dot_rev.get_merge_state()?;
                    let mut all_resolved = true;

                    // Clone conflicts for iteration
                    let conflicts_to_process: Vec<_> = merge_state.merge_result.conflicts.clone();

                    // Process each conflict with the given strategy
                    for conflict in conflicts_to_process {
                        let resolver = ConflictResolver::new(&store, conflict.clone(), &merge_state.current_branch, &merge_state.merge_branch)?;

                        // Try to apply the strategy
                        match resolver.resolve_with_strategy(&mut store, strategy)? {
                            Some(resolved_id) => {
                                // Update the conflict in the merge result
                                for c in merge_state.merge_result.conflicts.iter_mut() {
                                    if c.path == conflict.path {
                                        c.resolved = true;
                                        c.resolution_id = Some(resolved_id);
                                        break;
                                    }
                                }

                                println!("Resolved: {} ({})", conflict.path.display(), conflict.conflict_type);
                            },
                            None => {
                                // Strategy couldn't be applied
                                all_resolved = false;
                                println!("Could not resolve: {} ({})", conflict.path.display(), conflict.conflict_type);
                            }
                        }
                    }

                    // If all conflicts were resolved, create the merge snapshot
                    if all_resolved {
                        println!("\n{}", "All conflicts resolved automatically!".green().bold());

                        // Create the merge snapshot
                        let merge_msg = message.unwrap_or_else(||
                            format!("Merge branch '{}' into {} (strategy: {})",
                                merge_branch,
                                current_branch,
                                if strategy == merge::MergeStrategy::Ours { "ours" } else { "theirs" }
                            )
                        );

                        let snapshot_id = merge_state.merge_result.create_snapshot(&mut store, merge_msg)?;

                        // Update the branch pointer
                        dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;
                        refresh_worktree_to_snapshot(&dot_rev, &mut store, snapshot_id)?;

                        // Clear the merge state
                        dot_rev.clear_merge_state()?;

                        println!("Merge completed successfully: created snapshot {}",
                            snapshot_id.to_string().cyan());
                        return Ok(());
                    } else {
                        // Not all conflicts could be resolved automatically
                        dot_rev.save_merge_state(&merge_state)?;
                        return Err(AppError::MergeConflicts(merge_state.merge_result.conflicts.clone()));
                    }
                } else {
                    // In non-interactive mode, just report the conflicts
                    return Err(AppError::MergeConflicts(merge_result.conflicts));
                }
            }

            // No conflicts, create the merge snapshot directly
            let merge_msg = message.unwrap_or_else(||
                format!("Merge branch '{}' into {}", merge_branch, current_branch)
            );

            if let Some(merged_dir) = merge_result.merged_directory {
                // Save the merged directory
                let dir_id = store.insert_json(&merged_dir)?;

                // Create a merge snapshot
                let mut parents = BTreeSet::new();
                parents.insert(ours_id);
                parents.insert(theirs_id);

                let snapshot = SnapShot {
                    message: merge_msg,
                    directory: dir_id,
                    previous: parents,
                };

                // Save the snapshot
                let snapshot_id = store.insert_json(&snapshot)?;

                // Update the branch pointer
                dot_rev.set_branch_snapshot_id(&current_branch, snapshot_id)?;

                // Write the new directory to the working directory
                let cwd = dot_rev.work_dir().to_path_buf();
                merged_dir.write(&store, &cwd, false)
                    .map_err(|e| AppError::FailedToRestoreFiles(format!("{:?}", e)))?;

                println!("Merge completed successfully: created snapshot {}",
                    snapshot_id.to_string().cyan());
                Ok(())
            } else {
                Err(AppError::Other("Unexpected error: Merge succeeded but no merged directory available".to_string()))
            }
        },

        Ignore { pattern, remove } => {
            let (dot_rev, _branch) = get_repository()?;
            let mut ignores = dot_rev.ignores()?;

            // Use interactive mode if flag is set and no explicit arguments are provided
            if interactive && pattern.is_none() && !remove {
                return interactive_ignore_management(&dot_rev, ignores);
            }

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
                    if interactive {
                        // In interactive mode, show a list of patterns to remove
                        if ignores.patterns.is_empty() {
                            println!("{}", "No patterns to remove".yellow());
                            return Ok(());
                        }

                        let theme = ColorfulTheme::default();
                        let options: Vec<&String> = ignores.patterns.iter().collect();
                        let selection = Select::with_theme(&theme)
                            .with_prompt("Select pattern to remove")
                            .default(0)
                            .items(&options)
                            .interact()
                            .map_err(|_| AppError::Other("Failed to get user input".to_string()))?;

                        let pattern = ignores.patterns.remove(selection);
                        let new_ignores = Ignores::new(ignores.patterns);
                        dot_rev.set_ignores(&new_ignores)?;
                        println!("Removed '{}' from ignore patterns", pattern.red());
                    } else {
                        return Err(AppError::Other(
                            "no pattern given to remove.\nUsage: revtool ignore --remove <pattern>"
                                .to_string(),
                        ));
                    }
                }
            }

            Ok(())
        },
        Reset { delete_absent, force } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;

            // Reset discards uncommitted changes to tracked files, and with
            // --delete-absent it also deletes untracked files. Both are
            // irreversible, so require --force rather than doing it silently.
            if !force {
                let dirty = uncommitted_changes(&dot_rev, &mut store)?;
                let deletes_untracked = delete_absent;
                if !dirty.is_empty() || deletes_untracked {
                    let mut msg = String::from("'reset' discards uncommitted work:\n");
                    for path in &dirty {
                        msg.push_str(&format!("  modified/absent: {}\n", path.display()));
                    }
                    if deletes_untracked {
                        msg.push_str("  --delete-absent will also remove every untracked file\n");
                    }
                    msg.push_str("\nRe-run with --force to confirm");
                    return Err(AppError::Other(msg));
                }
            }

            let snapshot_id = dot_rev.branch_snapshot_id(&branch)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let directory: Directory = store.read_json(snapshot.directory)?;

            // Restore files from snapshot
            let cwd = dot_rev.work_dir().to_path_buf();
            directory.write(&store, &cwd, delete_absent)
                .map_err(|e| AppError::FailedToResetFiles(format!("{:?}", e)))?;

            println!("Reset to the last snapshot on branch '{}' ({})",
                branch.green().bold(),
                snapshot_id.to_string().cyan());
            Ok(())
        }
        Diff { first_ref, second_ref, content, context, no_color } => {
            let (dot_rev, current_branch) = get_repository()?;
            let store = dot_rev.store()?;

            // Parse snapshot references
            use std::str::FromStr;
            use lib::snapshot_ref::SnapshotRef;

            // Handle the various possible combinations of first_ref and second_ref
            let (source_ref, target_ref) = match (first_ref, second_ref) {
                (Some(first), Some(second)) => {
                    // Both refs provided: first is source, second is target
                    let source = SnapshotRef::from_str(&first)
                        .map_err(|e| AppError::Other(format!("Invalid first snapshot reference: {}", e)))?;
                    let target = SnapshotRef::from_str(&second)
                        .map_err(|e| AppError::Other(format!("Invalid second snapshot reference: {}", e)))?;
                    (source, target)
                },
                (Some(first), None) => {
                    // Only first ref provided: current branch is source, first is target
                    let source = SnapshotRef::Branch(current_branch);
                    let target = SnapshotRef::from_str(&first)
                        .map_err(|e| AppError::Other(format!("Invalid snapshot reference: {}", e)))?;
                    (source, target)
                },
                (None, Some(second)) => {
                    // Only second ref provided: second is source, current branch is target
                    let source = SnapshotRef::from_str(&second)
                        .map_err(|e| AppError::Other(format!("Invalid snapshot reference: {}", e)))?;
                    let target = SnapshotRef::Branch(current_branch);
                    (source, target)
                },
                (None, None) => {
                    // No refs provided - show help info about the new syntax
                    println!("Diff command requires at least one snapshot reference.");
                    println!("Example usage:");
                    println!("  revtool diff branch_name           # Compare current branch with branch_name");
                    println!("  revtool diff HEAD~1                # Compare current branch with its parent");
                    println!("  revtool diff abc123                # Compare current branch with snapshot abc123");
                    println!("  revtool diff branch1 branch2       # Compare branch1 with branch2");
                    println!("  revtool diff HEAD~1 feature        # Compare parent of HEAD with feature branch");
                    println!("\nUse 'revtool usage diff' for more examples.");
                    return Ok(());
                }
            };

            // Resolve snapshot references to actual snapshot IDs
            let source_id = source_ref.resolve(&dot_rev)
                .map_err(|e| AppError::Other(format!("Failed to resolve source reference: {}", e)))?;
            let target_id = target_ref.resolve(&dot_rev)
                .map_err(|e| AppError::Other(format!("Failed to resolve target reference: {}", e)))?;

            // Generate snapshot diff
            let snapshot_diff = lib::snapshot_diff::SnapShotDiff::generate(
                &store,
                source_id,
                target_id,
                content
            ).map_err(|e| AppError::Other(format!("Failed to generate diff: {:?}", e)))?;

            // Import the required types
            use lib::diff_format;

            // Create format options based on user preferences
            let format_options = diff_format::FormatOptions {
                use_color: !no_color,
                show_content: content,
                context_lines: context,
                show_stats: true,
            };

            // Format the diff with colors
            let formatter = diff_format::SnapShotDiffFormatter::new(&snapshot_diff, format_options);

            println!("{}", formatter);

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

        Status { content, context, no_color } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let old_tip: ObjectId = dot_rev.branch_snapshot_id(&branch)?;
            let ignores: Ignores = dot_rev.ignores()?;
            let cwd = dot_rev.work_dir().to_path_buf();

            // Calculate directory diff with or without content depending on options
            let directory = Directory::new(cwd.as_path(), &ignores, &mut store)
                .map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;
            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;

            let diff = if content {
                old_directory.diff_with_content(&directory, true, &store)
            } else {
                old_directory.diff(&directory)
            };

            println!("On branch {}", branch.green().bold());

            let merge_in_progress = dot_rev.is_merge_in_progress()?;
            if merge_in_progress {
                println!("\n{}", "You are in the middle of a merge.".yellow().bold());
                println!(
                    "  Resolve conflicts, then run \"{}\", or \"{}\" to cancel.",
                    "revtool merge --continue".cyan(),
                    "revtool merge --abort".cyan()
                );
            }

            if diff.added.is_empty() && diff.deleted.is_empty() && diff.modified.is_empty() {
                println!("{}", "Working tree clean, nothing to snapshot".green());
            } else {
                println!("\n{}", "Changes not yet snapped:".yellow().bold());
                // Don't advise snapshotting mid-merge; that would commit markers.
                if !merge_in_progress {
                    println!("  (use \"{}\" to create a new snapshot)\n", "revtool snap -m <message>".cyan());
                }

                // Import the required types locally
                use lib::diff_format;

                // Use our formatter for consistent display
                let format_options = diff_format::FormatOptions {
                    use_color: !no_color,
                    show_content: content,
                    context_lines: context,
                    show_stats: true,
                };

                let formatter = diff_format::DiffFormatter::new(&diff, format_options);
                println!("{}", formatter);
            }
            Ok(())
        },

        Log { limit } => {
            // Repository existence check is now done at the beginning of the function
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
        Checkout { branch, create, force } => {
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

            // If the branch doesn't exist, only create it on explicit intent
            // (`-b`, or a confirmation in interactive mode). A bare typo used to
            // silently create a phantom branch; now it errors and points at how
            // to create one on purpose.
            if !dot_rev.branch_exists(&branch_to_checkout)? {
                let should_create = create
                    || (interactive
                        && Confirm::with_theme(&ColorfulTheme::default())
                            .with_prompt(format!(
                                "Branch '{}' doesn't exist. Create it?",
                                branch_to_checkout
                            ))
                            .default(true)
                            .interact()
                            .unwrap_or(false));
                if should_create {
                    println!("Creating new branch '{}'", branch_to_checkout.green().bold());
                    dot_rev.create_branch(&branch_to_checkout)?;
                } else if interactive {
                    println!("Branch creation aborted.");
                    return Ok(());
                } else {
                    return Err(AppError::Other(format!(
                        "Branch '{0}' does not exist. Use 'revtool checkout -b {0}' to create it, \
                         or 'revtool branch {0}' first",
                        branch_to_checkout
                    )));
                }
            }

            // Protect uncommitted work: switching overwrites tracked files.
            ensure_clean_or_forced(&dot_rev, &mut store, force, "switching branches")?;

            // Capture the branch we're leaving so we can remove files that exist
            // only on it, then lay down the target branch's files.
            let current_snapshot_id = dot_rev.branch_snapshot_id(&current_branch)?;
            let current_snap: SnapShot = store.read_json(current_snapshot_id)?;
            let current_tree: Directory = store.read_json(current_snap.directory)?;

            let snapshot_id = dot_rev.branch_snapshot_id(&branch_to_checkout)?;
            let snapshot: SnapShot = store.read_json(snapshot_id)?;
            let target_tree: Directory = store.read_json(snapshot.directory)?;

            let cwd = dot_rev.work_dir().to_path_buf();
            remove_tracked_absent(&current_tree, &target_tree, &cwd)?;
            target_tree.write(&store, &cwd, false)
                .map_err(|e| AppError::FailedToRestoreFiles(format!("{:?}", e)))?;

            // Only move the branch pointer once the working tree is in place.
            dot_rev.set_branch(&branch_to_checkout)?;

            println!("Switched to branch '{}' (snapshot: {})",
                branch_to_checkout.green().bold(),
                snapshot_id.to_string().cyan());
            Ok(())
        }
        Changes { content, json, context, no_color } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            let old_tip = dot_rev.branch_snapshot_id(&branch)?;
            let ignores = dot_rev.ignores()?;

            let directory = Directory::new(
                dot_rev.work_dir(),
                &ignores,
                &mut store
            ).map_err(|e| AppError::FailedToReadDirectory(format!("{:?}", e)))?;

            let snapshot: SnapShot = store.read_json(old_tip)?;
            let old_directory: Directory = store.read_json(snapshot.directory)?;

            // Generate diff with or without content
            let diff = if content {
                old_directory.diff_with_content(&directory, true, &store)
            } else {
                old_directory.diff(&directory)
            };

            if json {
                // Output as JSON
                serde_json::to_writer_pretty(stdout(), &diff)
                    .map_err(|e| AppError::FailedToOutputChanges(format!("{}", e)))?;
            } else {
                // Import the required types locally
                use lib::diff_format;

                // Use our formatter for consistent display
                let format_options = diff_format::FormatOptions {
                    use_color: !no_color,
                    show_content: content,
                    context_lines: context,
                    show_stats: true,
                };

                let formatter = diff_format::DiffFormatter::new(&diff, format_options);
                println!("{}", formatter);
            }

            Ok(())
        }
        Snap { message } => {
            let (dot_rev, branch) = get_repository()?;
            let mut store = dot_rev.store()?;

            // Refuse to snapshot in the middle of a merge: the working tree
            // still has conflict markers, and committing them corrupts history.
            if dot_rev.is_merge_in_progress()? {
                return Err(AppError::Other(
                    "a merge is in progress. Resolve the conflicts and run \
                     'revtool merge --continue', or 'revtool merge --abort' to cancel"
                        .to_string(),
                ));
            }

            let old_tip = dot_rev.branch_snapshot_id(&branch)?;
            let ignores = dot_rev.ignores()?;

            // Create a snapshot of the current directory
            let directory = Directory::new(
                dot_rev.work_dir(),
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
            let cwd = current_dir()?;
            let rev_path = cwd.join(".rev");

            // Don't silently claim success (or nest a repo) when one already
            // exists here or in a parent directory.
            if rev_path.exists() {
                return Err(AppError::Other(format!(
                    "A repository already exists at {}",
                    rev_path.display()
                )));
            }
            if let Ok(existing) = DotRev::here() {
                return Err(AppError::Other(format!(
                    "Already inside a repository rooted at {}. \
                     Initialising here would nest a second repository",
                    existing.work_dir().display()
                )));
            }

            DotRev::init(rev_path)?;
            println!("{}", "Initialized empty revision control repository in .rev/".green().bold());
            Ok(())
        }
        
        Remote { cmd } => {
            let (dot_rev, _) = get_repository()?;
            
            match cmd {
                Some(RemoteCommand::Add { name, url }) => {
                    let remote = RemoteConfig { name: name.clone(), url: url.clone() };
                    dot_rev.add_remote(remote)?;
                    println!("Added remote '{}' with URL: {}", name.green().bold(), redact_url_credentials(&url));
                    Ok(())
                },
                Some(RemoteCommand::Remove { name }) => {
                    // Don't report success for a remote that was never there.
                    if dot_rev.get_remote(&name)?.is_none() {
                        return Err(AppError::Other(format!("No remote named '{name}'")));
                    }
                    dot_rev.remove_remote(&name)?;
                    println!("Removed remote '{}'", name.red().bold());
                    Ok(())
                },
                None => {
                    // List remotes
                    let remotes = dot_rev.remotes()?;
                    if remotes.is_empty() {
                        println!("No remotes configured");
                    } else {
                        println!("{}", "Configured remotes:".green().bold());
                        for remote in remotes {
                            println!("  {} -> {}", remote.name.cyan(), redact_url_credentials(&remote.url));
                        }
                    }
                    Ok(())
                }
            }
        }
        
        Push { remote, branch, force } => {
            let (dot_rev, current_branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            
            // Get the branch to push
            let branch_to_push = branch.as_ref().unwrap_or(&current_branch);
            
            // Get remote configuration
            let remote_config = dot_rev.get_remote(&remote)?
                .ok_or_else(|| AppError::Other(format!("Remote '{}' not found. Use 'revtool remote add' to configure it.", remote)))?;
            
            // Create HTTP client, authenticating with REVTOOL_TOKEN if set.
            let token = std::env::var("REVTOOL_TOKEN").ok();
            let client = HttpRemoteClient::with_token(remote_config.url.clone(), token)
                .map_err(|e| AppError::Other(format!("Failed to connect to remote: {}", e)))?;
            
            // Get the local snapshot to push
            let local_snapshot_id = dot_rev.branch_snapshot_id(branch_to_push)?;
            
            println!("Pushing branch '{}' to remote '{}'...", branch_to_push.cyan(), remote.cyan());
            
            // Push the branch
            sync::push_branch(&client, &mut store, branch_to_push, local_snapshot_id, force)
                .map_err(|e| AppError::Other(format!("Push failed: {}", e)))?;
            
            println!("{}", "Push completed successfully!".green().bold());
            Ok(())
        }
        
        Pull { remote, branch } => {
            let (dot_rev, current_branch) = get_repository()?;
            let mut store = dot_rev.store()?;
            
            // Get the branch to pull
            let branch_to_pull = branch.as_ref().unwrap_or(&current_branch);
            
            // Get remote configuration
            let remote_config = dot_rev.get_remote(&remote)?
                .ok_or_else(|| AppError::Other(format!("Remote '{}' not found. Use 'revtool remote add' to configure it.", remote)))?;
            
            // Create HTTP client, authenticating with REVTOOL_TOKEN if set.
            let token = std::env::var("REVTOOL_TOKEN").ok();
            let client = HttpRemoteClient::with_token(remote_config.url.clone(), token)
                .map_err(|e| AppError::Other(format!("Failed to connect to remote: {}", e)))?;
            
            println!("Pulling branch '{}' from remote '{}'...", branch_to_pull.cyan(), remote.cyan());
            
            // Pull the branch
            match sync::pull_branch(&client, &mut store, branch_to_pull)
                .map_err(|e| AppError::Other(format!("Pull failed: {}", e)))? {
                Some(new_snapshot_id) => {
                    // Check if this is a new branch
                    let is_new_branch = !dot_rev.branch_exists(branch_to_pull)?;
                    
                    // Update the local branch pointer (this creates the branch if it doesn't exist)
                    dot_rev.set_branch_snapshot_id(branch_to_pull, new_snapshot_id)?;
                    
                    // If we pulled the current branch, update the working directory
                    if branch_to_pull == &current_branch {
                        let snapshot: SnapShot = store.read_json(new_snapshot_id)?;
                        let directory: Directory = store.read_json(snapshot.directory)?;
                        
                        let cwd = dot_rev.work_dir().to_path_buf();
                        directory.write(&store, &cwd, false)
                            .map_err(|e| AppError::FailedToRestoreFiles(format!("{:?}", e)))?;
                        
                        println!("Updated working directory to match remote snapshot");
                    }
                    
                    if is_new_branch {
                        println!("{}", format!("Created new branch '{}' from remote", branch_to_pull).green().bold());
                    }
                    
                    println!("{}", format!("Pull completed successfully! Branch '{}' updated to {}", 
                        branch_to_pull, new_snapshot_id.to_string().cyan()).green().bold());
                },
                None => {
                    println!("{}", format!("Branch '{}' not found on remote '{}'", branch_to_pull, remote).yellow());
                }
            }
            
            Ok(())
        }
        
        Serve { port, host, token } => {
            let (dot_rev, _) = get_repository()?;

            // Token from --token, falling back to the REVTOOL_TOKEN env var so
            // it need not appear in the process list.
            let token = token.or_else(|| std::env::var("REVTOOL_TOKEN").ok());

            // Create the server
            let server = HttpRemoteServer::with_token(dot_rev.root().clone(), token.clone())
                .map_err(|e| AppError::Other(format!("Failed to create server: {}", e)))?;

            let addr = format!("{}:{}", host, port);

            // Create a runtime for the async server
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| AppError::Other(format!("Failed to create runtime: {}", e)))?;

            // Without a token the server is unauthenticated: anyone who can
            // reach the port can read every object and push to any branch. That
            // is fine on loopback, but binding to a routable address exposes the
            // repo to the whole network, so warn loudly.
            let is_loopback = host == "127.0.0.1" || host == "localhost" || host == "::1";
            if token.is_none() {
                if !is_loopback {
                    eprintln!(
                        "{} serving on {} with no authentication. Anyone who can reach \
                         this address can read and overwrite this repository. Pass --token \
                         (or set REVTOOL_TOKEN), or only serve on a trusted network.",
                        "WARNING:".yellow().bold(),
                        addr
                    );
                }
            } else {
                println!("Requiring a bearer token for all requests.");
            }

            println!("Starting repository server on {}...", addr.cyan().bold());
            println!("Other users can add this as a remote with:");
            println!("  revtool remote add origin http://{}", addr);
            
            // Block on the server
            runtime.block_on(async {
                server.run(&addr).await
                    .map_err(|e| AppError::Other(format!("Server error: {}", e)))
            })?;
            
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
