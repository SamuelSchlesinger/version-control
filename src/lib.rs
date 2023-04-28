#![feature(fs_try_exists)]

//! # Revision Control
//!
//! This is an implementation of a basic revision control system.

mod hex;

/// Hash-based binary object identifier.
pub mod object_id;
/// Content addressible store API using the
/// identifier defined in [`object_id`].
pub mod object_store;
