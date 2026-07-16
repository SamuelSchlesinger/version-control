# RevTool: A Modern Version Control System

[![Rust](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml/badge.svg)](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml)

RevTool is a lightweight, approachable version control system implemented from scratch in Rust. It focuses on simplicity, performance, and modern content-addressable storage principles while providing a familiar workflow for developers used to Git. 

The project aims to deliver a more understandable and transparent approach to version control, making it both a practical tool and an educational resource for understanding how version control systems work.

**Why RevTool?**
- **Simplicity**: Clear conceptual model with straightforward commands
- **Modern design**: Built using Rust and BLAKE3 cryptographic hashing
- **Approachable code**: Well-organized architecture for learning and extending
- **Interactive workflows**: Guided operations for complex tasks like merges
- **Familiar feel**: Git-like concepts with more intuitive naming and behavior

## Table of Contents

- [Quick Start](#quick-start)
- [Key Differences from Git](#key-differences-from-git)
- [Installation](#installation)
- [Feature Highlights](#feature-highlights)
- [Command Reference](#command-reference)
- [Common Workflows](#common-workflows)
- [Advanced Features](#advanced-features)
- [Architecture](#architecture)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

## Quick Start

Get up and running with RevTool in minutes:

```bash
# Install RevTool
git clone https://github.com/SamuelSchlesinger/version-control.git
cd version-control
cargo build --release
export PATH="$PATH:$(pwd)/target/release"  # Add to your path

# Start your project
mkdir my-project
cd my-project
revtool init
echo "# My Project" > README.md
revtool status
revtool snap -m "Initial commit"

# Create and use a feature branch
revtool branch feature
revtool checkout feature
# Make changes...
revtool status
revtool snap -m "Add new functionality"

# Merge changes back to the dev branch
revtool checkout dev
revtool merge feature -m "Merge feature branch"
```

## Key Differences from Git

RevTool builds on Git's concepts while simplifying and modernizing several aspects:

| Aspect | Git | RevTool | Benefit |
|--------|-----|---------|---------|
| **Terminology** | "commit" | "snapshot" | More intuitive naming |
| **Hashing** | SHA-1 | BLAKE3 | Faster, more secure hashing |
| **Default Branch** | main/master | dev | Modern naming convention |
| **Merge Resolution** | Various tools | Built-in interactive | Guided conflict resolution |
| **Repository** | .git | .rev | Fresh implementation |
| **Object Model** | Complex | Simplified | Easier to understand |
| **Implementation** | C | Rust | Memory safety, modern language |

While Git offers more advanced features and widespread adoption, RevTool provides a cleaner, more approachable design that's ideal for:
- Learning how version control works
- Personal projects where simplicity is valued
- Educational environments
- Projects that benefit from interactive merge resolution

**Performance:** RevTool's simpler object model makes whole-tree operations
substantially faster — snapshotting a ~72 MB / 3,000-file tree is measured at
about **6.6× faster than `git commit`** (it skips zlib compression and delta
encoding). A working-tree stat index (like git's) keeps incremental snapshots
fast too — re-snapshotting after a one-file change is competitive with, and in
our measurement slightly faster than, `git add && git commit`. The index is
made safe against silently missing an in-place edit by keying on ctime (which
the OS bumps on any change and cannot be forged backward), with `snap --rehash`
as an escape hatch. See [BENCHMARKS.md](BENCHMARKS.md) for the full methodology,
numbers, and the safety design.

## Installation

### From Source

Clone the repository and build with Cargo:

```bash
git clone https://github.com/SamuelSchlesinger/version-control.git
cd version-control
cargo build --release
```

The binary will be available at `target/release/revtool`.

For convenience, you can add it to your PATH:

```bash
# Temporary (current session only)
export PATH="$PATH:$(pwd)/target/release"

# Permanent (add to your .bashrc, .zshrc, etc.)
echo 'export PATH="$PATH:/path/to/version-control/target/release"' >> ~/.bashrc
source ~/.bashrc
```

### Prerequisites

- Rust (1.54 or later recommended)
- Cargo

### Verifying Installation

To verify the installation:

```bash
revtool --version
```

## Feature Highlights

RevTool offers a range of powerful features designed for modern version control workflows:

### Core Features

- **Content-addressable storage** with BLAKE3 cryptographic hashing
  - Efficient storage with automatic deduplication
  - Cryptographic verification of content integrity
  - Fast object retrieval

- **Branch-based workflow** with intuitive semantics
  - Simple branch creation and switching
  - Support for multiple development lines
  - Familiar mental model for Git users

- **Snapshot-based versioning** with complete history
  - Directed acyclic graph (DAG) for history tracking
  - Support for merge commits with multiple parents
  - Efficient storage that only tracks changes

- **Flexible file ignoring** with gitignore-compatible patterns
  - Use familiar glob syntax
  - Automatically ignore common build artifacts
  - Interactive pattern management

- **Comprehensive diffing capabilities**
  - Structural diffs (files added/removed/modified)
  - Content-level diffs (line-by-line changes)
  - Rich formatting with context

### Advanced Features

- **Robust merge capabilities**
  - Three-way merging with conflict detection
  - Interactive conflict resolution
  - Multiple automatic merge strategies
  - Custom editor support for conflict resolution

- **Flexible snapshot references**
  - Support for branch names, direct IDs
  - Relative references like `HEAD~N`
  - Shortened hash prefixes

- **Interactive mode** for guided operations
  - Simplified workflows for complex tasks
  - Clear conflict resolution interfaces
  - Friendly confirmation prompts

## Command Reference

```
Usage: revtool [OPTIONS] <COMMAND>

Commands:
  usage     Display help information and usage examples
  ignore    Manage the ignore patterns
  init      Initialize a brand new revision control repository
  diff      Check the difference between snapshots
  changes   Shows files and directories changed since the latest snapshot
  status    Show working tree status
  snap      Take a new snapshot (similar to git commit)
  merge     Merge changes from another branch into the current branch
  checkout  Switch to a branch
  branch    Create or list branches
  tag       Create, list, or delete tags (immutable release markers)
  reset     Reset all files to the last snapshot on this branch
  log       Show commit logs
  remote    Manage remote repositories
  push      Push changes to a remote repository
  pull      Pull changes from a remote repository
  serve     Start an HTTP server to host the repository
  help      Print this message or the help of the given subcommand(s)

Options:
  -i, --interactive  Use interactive mode with prompts and confirmations
  -h, --help         Print help
  -V, --version      Print version
```

RevTool can be run from any subdirectory of a repository, like git.

#### Safety flags

Commands that overwrite the working tree refuse to discard uncommitted work
unless you opt in:

```bash
revtool checkout other            # refused if you have uncommitted changes
revtool checkout other --force    # discard local changes and switch
revtool checkout -b new-branch    # create the branch and switch (like git switch -c)
revtool reset --force             # restore tracked files, discarding local edits
revtool reset --force --delete-absent  # also remove untracked files
```

### Key Commands

#### Initialize a Repository
```bash
revtool init
```
Creates a new `.rev` directory to track your files.

#### Checking Status
```bash
revtool status
revtool status --content  # Shows content-level changes
```
Shows which files have been added, modified, or deleted.

#### Taking Snapshots (Committing)
```bash
revtool snap -m "Your commit message"
revtool snap -i  # Interactive mode with guided prompts
```
Records the current state of your files.

#### Branch Management
```bash
revtool branch                 # List branches
revtool branch feature-name    # Create a new branch
revtool checkout feature-name  # Switch to a branch
```

#### Comparing Versions
```bash
revtool diff main feature      # Compare two branches
revtool diff HEAD~1            # Compare with previous snapshot
revtool diff --content main    # Show content-level differences
```

#### Viewing History
```bash
revtool log         # Show recent snapshots
revtool log -l 5    # Show 5 most recent snapshots
```

#### Ignoring Files
```bash
revtool ignore                # List current patterns
revtool ignore "*.log"        # Add a pattern
revtool ignore --remove "*.log"  # Remove a pattern
revtool ignore -i             # Interactive pattern management
```

#### Tagging Releases
```bash
revtool tag v1.0            # Tag the current snapshot as v1.0
revtool tag                 # List all tags
revtool diff v1.0           # Diff the working tree against a tagged snapshot
revtool tag --delete v1.0   # Delete a tag
```
Tags are immutable named references to a snapshot, usable anywhere a snapshot
reference is accepted.

#### Resetting Files
```bash
revtool reset --force                 # Restore tracked files to the last snapshot
revtool reset --force --delete-absent # Also delete untracked files
```
`reset` discards uncommitted work, so it requires `--force`.

#### Working with Remotes
```bash
revtool serve --port 8080             # Host the current repo over HTTP
revtool remote add origin http://host:8080
revtool push origin dev               # Upload the dev branch
revtool pull origin dev               # Download the dev branch
```

To require authentication, start the server with a token (via `--token` or the
`REVTOOL_TOKEN` environment variable); clients then supply the same token in
`REVTOOL_TOKEN`:

```bash
revtool serve --port 8080 --token "$SECRET"   # server rejects requests without it
REVTOOL_TOKEN="$SECRET" revtool pull origin dev
```

Without a token the server is unauthenticated, so bind it to a trusted network
only (it defaults to `127.0.0.1`).

## Common Workflows

### Starting a New Project

```bash
# Initialize a repository
revtool init

# Create initial files
echo "# My Project" > README.md

# Check status
revtool status

# Create first snapshot
revtool snap -m "Initial commit"
```

### Making Changes

```bash
# Edit files...

# Check what's changed
revtool status
revtool status --content  # See the actual content changes

# Record the changes
revtool snap -m "Add feature X"
```

### Working with Branches

The default branch is `dev`. `checkout` switches to an existing branch; use
`-b` to create one (checking out a name that doesn't exist is an error, so a
typo can't silently make a stray branch).

```bash
# Create and switch to a feature branch
revtool checkout -b new-feature

# Make changes and commit them
# ... edit files ...
revtool snap -m "Work on new feature"

# Compare with the dev branch
revtool diff dev

# Switch back to dev
revtool checkout dev
```

### Merging Branches

```bash
# Work on a feature branch
revtool checkout -b feature-branch
# ... make changes and create snapshots ...

# Switch to target branch and merge
revtool checkout dev
revtool merge feature-branch -m "Merge feature-branch into dev"

# If there are conflicts, you'll be prompted to resolve them
# After resolving conflicts manually
revtool merge --continue

# If you want to abort a merge with conflicts
revtool merge --abort
```

## Advanced Features

### Merge Strategies

RevTool provides several advanced merge capabilities to handle different scenarios:

```bash
# Automatically resolve all conflicts by taking our (current branch) version
revtool merge feature-branch --strategy ours

# Automatically resolve all conflicts by taking their (merging branch) version
revtool merge feature-branch --strategy theirs

# Default behavior, requires manual resolution
revtool merge feature-branch --strategy normal
```

In interactive mode (`-i`), even with a strategy specified, you'll still be shown the conflicts that were automatically resolved.

### Custom Editor for Conflict Resolution

Specify your preferred editor for resolving merge conflicts:

```bash
# Use vim to edit conflict files
revtool merge feature-branch --editor vim

# Use VS Code to edit conflict files
revtool merge feature-branch --editor "code -w"
```

The editor will be launched automatically when you choose to manually edit a conflict.

### Interactive Mode

Most commands support an interactive mode with `-i` that guides you through the process:

```bash
revtool snap -i       # Interactive snapshot creation
revtool checkout -i   # Interactive branch selection
revtool ignore -i     # Interactive ignore management
```

## Architecture

RevTool follows a design philosophy centered on simplicity, understandability, and functional purity. Core principles include:

- **Content-addressable storage** as the foundation
- **Immutable objects** for reliability
- **Clean separation of concerns** in modular components
- **Type safety** through Rust's strong typing system
- **Explicit over implicit** in operations and commands

### Core Components

1. **`ObjectId` (`object_id.rs`)**
   - Unique identifier based on content hash (BLAKE3)
   - Used to address all content in the system

2. **`ObjectStore` (`object_store.rs`)**
   - Content-addressable storage interface
   - Provides filesystem (`DirectoryObjectStore`) and in-memory implementations
   - Automatically deduplicates identical content

3. **`Directory` (`directory.rs`)**
   - Represents a directory tree structure
   - Maps file paths to their content hashes
   - Includes diffing capabilities for comparing directories

4. **`SnapShot` (`snapshot.rs`)**
   - Represents a point-in-time version of the repository
   - Links to a directory structure and parent snapshot(s)
   - Forms nodes in the version history graph

5. **`DotRev` (`dot_rev.rs`)**
   - Manages the `.rev` directory structure
   - Provides repository metadata operations
   - Handles branch management

6. **Content Diffing (`content_diff.rs`)**
   - Implements line-based diffing for text files
   - Uses a simplified Myers diff algorithm

7. **Snapshot References (`snapshot_ref.rs`)**
   - Flexible syntax for referencing snapshots
   - Supports branch names, HEAD, relative refs (HEAD~1), and direct IDs

### Repository Structure

The `.rev` directory contains:

```
.rev/
  ├── store/         # Content-addressable object store
  │   └── xx/        # Two-digit prefix subdirectories for efficient lookup
  │       └── yy...  # Object contents stored by remaining hash digits
  ├── branches/      # Branch references
  │   ├── main       # Each file contains a snapshot ID
  │   └── dev        # ...for the tip of the branch
  ├── branch         # Text file containing current branch name
  └── ignores        # JSON file with patterns for ignored files
```

All objects (files, directories, snapshots) are stored in the content-addressable store and referenced by their hash, ensuring integrity and deduplication.

## Troubleshooting

### Common Issues

**Issue**: "Repository not initialized" error
**Solution**: Run `revtool init` in the root directory of your project.

**Issue**: Changes not showing up in status
**Solution**: Check if the files are in the ignore list with `revtool ignore`. Files matching ignore patterns won't appear in status.

**Issue**: Failed merge with conflicts
**Solution**: 
1. Resolve conflicts in the marked files
2. Run `revtool merge --continue`
3. If you want to start over, use `revtool merge --abort`

**Issue**: "Branch not found" error
**Solution**: List available branches with `revtool branch` and check spelling. If needed, create the branch first.

### Debugging Tips

- Use `revtool status --content` for detailed change information
- Examine the `.rev` directory structure for troubleshooting repository issues
- Try interactive mode (`-i`) for complex operations to see more information

### Getting Help

- Use `revtool usage <command>` for detailed help on a specific command
- Review this README for workflow guidance
- Submit issues via GitHub for bugs or feature requests

## Contributing

Contributions are welcome! To get started:

1. Fork the repository
2. Create a new branch for your feature or fix
3. Make your changes
4. Submit a pull request

Please ensure your code follows the existing style patterns and includes appropriate tests.

### Development Workflow

1. Clone your fork: `git clone https://github.com/your-username/version-control.git`
2. Add upstream: `git remote add upstream https://github.com/SamuelSchlesinger/version-control.git`
3. Create branch: `git checkout -b my-feature`
4. Make changes and test
5. Commit: `git commit -am "Add new feature"`
6. Push: `git push origin my-feature`
7. Create PR through GitHub

## License

See the [LICENSE](LICENSE) file for details.
