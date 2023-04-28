#![feature(fs_try_exists)]

//! # Revision Control
//!
//! This is an implementation of a basic revision control system.

mod hex;

/// A data structure representing a directory structure with
/// names of files pointing to an [`ObjectId`].
pub mod directory;
/// Hash-based binary object identifier.
pub mod object_id;
/// Content addressible store API using the [`ObjectId`].
pub mod object_store;
