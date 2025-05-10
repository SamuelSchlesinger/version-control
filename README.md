# Version Control

[![Rust](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml/badge.svg)](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml)

A lightweight, flexible version control system implemented from scratch in Rust, focusing on simplicity, performance, and content-addressable storage principles. This project provides a Git-like workflow with a more approachable design.

## Features

- **Content-addressable storage** with BLAKE3 cryptographic hashing
- **Branch-based workflow** similar to Git but with simpler concepts
- **Snapshot-based versioning** with commit history
- **Flexible file ignoring** with gitignore-compatible pattern syntax
- **Rich diffing capabilities** including both structural and content-level differences
- **Interactive mode** for more guided operations
- **Clean, modular architecture** implemented in safe Rust

## Installation

Clone the repository and build with Cargo:

```bash
git clone https://github.com/SamuelSchlesinger/version-control.git
cd version-control
cargo build --release
```

The binary will be available at `target/release/revtool`.

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
  checkout  Switch to a branch
  branch    Create or list branches
  reset     Reset all files to the last snapshot on this branch
  log       Show commit logs
  help      Print this message or the help of the given subcommand(s)

Options:
  -i, --interactive  Use interactive mode with prompts and confirmations
  -h, --help         Print help
  -V, --version      Print version
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

#### Resetting Files
```bash
revtool reset                 # Reset to last snapshot
revtool reset --delete-absent # Reset and delete untracked files
```

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

```bash
# Create and switch to a feature branch
revtool branch new-feature
revtool checkout new-feature

# Make changes and commit them
# ... edit files ...
revtool snap -m "Work on new feature"

# Compare with main branch
revtool diff main

# Switch back to main
revtool checkout main
```

### Interactive Mode

Most commands support an interactive mode with `-i` that guides you through the process:

```bash
revtool snap -i       # Interactive snapshot creation
revtool checkout -i   # Interactive branch selection
revtool ignore -i     # Interactive ignore management
```

## Architecture

RevTool is built around several key components that work together:

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

## Implementation Details

### Snapshot History as a DAG

The version history is stored as a directed acyclic graph (DAG):
- Each snapshot can have multiple parent snapshots
- This supports merge operations (not yet fully implemented in CLI)
- The `previous` field in `SnapShot` maintains these relationships

### Efficient Storage

- Only stores changed files, not full copies
- Content-addressable storage automatically deduplicates content
- Directory structures are hierarchical and efficient

### Flexible References

You can reference snapshots in multiple ways:
- `branch_name` - Latest snapshot on a branch
- `HEAD` - Latest snapshot on current branch
- `HEAD~N` - N snapshots back from HEAD
- `branch_name~N` - N snapshots back from branch tip
- `abc123` - Snapshot ID (prefix or full hash)

## Contributing

Contributions are welcome! To get started:

1. Fork the repository
2. Create a new branch for your feature or fix
3. Make your changes
4. Submit a pull request

Please ensure your code follows the existing style patterns and includes appropriate tests.

## License

See the [LICENSE](LICENSE) file for details.