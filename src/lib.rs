//! # Revision Control
//!
//! A lightweight version control system written from scratch in Rust, focusing on
//! simplicity and performance. This library provides the core functionality for
//! tracking changes to files and directories, maintaining snapshot history, and
//! managing branches.
//!
//! ## Architecture
//!
//! The system is built around several key components:
//!
//! 1. **Content-addressable storage** - Files are stored and addressed by their content hash
//!    using the BLAKE3 hashing algorithm
//! 2. **Directory structure representation** - Complete directory trees are represented with
//!    file paths mapping to their content hashes
//! 3. **Snapshot history** - A directed acyclic graph (DAG) of snapshots that forms the version history
//! 4. **Branch management** - Separate development lines maintained as references to snapshots
//! 5. **Diffing** - Both structure-level (files added/removed/modified) and content-level
//!    (line-by-line changes) diffing capabilities between files and snapshots
//!
//! ## Usage
//!
//! This library is primarily used through the `revtool` CLI application, but
//! can also be used as a library for custom version control applications.

mod hex;

/// Represents directory trees with files mapped to their content hashes.
/// Provides functionality for diffing, writing, and creating directory structures.
pub mod directory;

/// Manages the `.rev` directory structure and provides convenience functions
/// for working with the repository metadata.
pub mod dot_rev;

/// Defines the `ObjectId` type which uniquely identifies binary content
/// using cryptographic hashing with BLAKE3.
pub mod object_id;

/// Provides a content-addressable storage system where objects are stored
/// and retrieved by their `ObjectId`. Includes both in-memory and persistent
/// filesystem implementations.
pub mod object_store;

/// Implements the `SnapShot` type which represents a point-in-time version
/// of a directory structure, forming nodes in the version history graph.
pub mod snapshot;

/// Provides functionality for diffing file contents at the line level,
/// showing what changed within files between different versions.
pub mod content_diff;

/// Implements functionality for comparing snapshots and generating detailed
/// diffs between repository states at different points in the version history.
pub mod snapshot_diff;

/// Provides utilities for formatting diffs in human-readable ways,
/// including colorized terminal output and context-sensitive displays.
pub mod diff_format;

/// Provides functionality for referencing snapshots with various syntaxes,
/// including direct IDs, HEAD, relative references (e.g., HEAD~1), and branch tips.
pub mod snapshot_ref;

// Test modules
#[cfg(test)]
mod tests;
