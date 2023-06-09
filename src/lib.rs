#![feature(fs_try_exists)]

//! # Revision Control
//!
//! This is an implementation of a basic revision control system.

mod hex;

/// A data structure representing a directory structure with
/// names of files and their `ObjectId`.
pub mod directory;
/// A .rev directory as well as associated convenience functions.
pub mod dot_rev;
/// Hash-based binary object identifier type called `ObjectId`.
pub mod object_id;
/// Content addressible store API using `ObjectId` as the address.
pub mod object_store;
/// A snapshot of a particular revision.
pub mod snapshot;
