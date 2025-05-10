# Version Control

[![Rust](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml/badge.svg)](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml)

A lightweight version control system written from scratch in Rust, focusing on
simplicity and performance.

## Features

- Content-addressable storage with BLAKE3 hashing
- Branch-based workflow similar to Git
- Snapshot-based versioning
- Configurable file ignoring
- Directory diffing to track changes

## Installation

Clone the repository and build with Cargo:

```bash
git clone https://github.com/SamuelSchlesinger/version-control.git
cd version-control
cargo build --release
```

The binary will be available at `target/release/revtool`.

## CLI Usage

```
Usage: revtool <COMMAND>

Commands:
  init      Initialize a brand new revision repository
  diff      Check the difference between this branch and another
  changes   Show files and directories changed since the latest snapshot
  snap      Take a new snapshot (commit changes)
  checkout  Switch to a different branch
  branch    Print out current branch name
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

### Examples

```bash
# Initialize a new repository
revtool init

# View current changes
revtool changes

# Create a snapshot with a message
revtool snap "Initial commit"

# Switch to a different branch
revtool checkout feature-branch

# Compare with another branch
revtool diff main
```

## How Does It Work?

Under the hood, it is using a content-addressed binary object store in
`.rev/store` using the [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) hash
function to identify binary objects using a type called `ObjectId`. Building on
top of that, we have a `Directory` data type which represents a directory tree
structure where each file's name is mapped to its `ObjectId`. We store this
type directly, encoded as prettified JSON, in the object store as a commitment
to a particular configuration or version.

A data type called `SnapShot` links directories together into a directed
acyclic graph (DAG), with each vertex having a message attached:

```rust
pub struct SnapShot {
  message: String,      // Commit message
  directory: ObjectId,  // Points to the directory structure
  previous: BTreeSet<ObjectId>, // Points to parent snapshots
}
```

We store this type directly in the object store as well, again as
prettified JSON. In the `.rev/branches` directory, we keep a file for each
branch with the `ObjectId` of a particular encoded `SnapShot`. In
`.rev/branch`, we keep the name of the current branch we're using.

When constructing a `Directory` from the current directory, we often don't want
to track certain files (e.g., build artifacts, logs). The `.rev/ignores` file
configures which paths we ignore. By default, it includes patterns like `.rev`,
`.git`, and various build artifact patterns.

## Repository Structure

Key files and directories in a `.rev` repository:

```
.rev/
  ├── store/         # Content-addressable object store
  ├── branches/      # Branch references
  ├── branch         # Current branch name
  └── ignores        # Patterns for ignored files
```

## Contributing

There are a number of issues on the GitHub repository. Please feel free to take
any and contribute. Pull requests are welcome!

To get started:
1. Fork the repository
2. Create a new branch for your feature or fix
3. Make your changes
4. Submit a pull request

## License

See the [LICENSE](LICENSE) file for details.
