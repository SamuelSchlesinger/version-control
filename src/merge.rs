use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    directory::{Diff, Directory, DirectoryEntry},
    dot_rev::InsertJson,
    object_id::ObjectId,
    object_store::ObjectStore,
    snapshot::SnapShot,
};

/// Error types for merge operations
#[derive(Debug)]
pub enum Error<Store: ObjectStore> {
    /// Object missing from store
    ObjectMissing(ObjectId),
    /// Store error
    Store(Store::Error),
    /// JSON parse error
    ParseError(serde_json::Error),
    /// I/O error
    IO(std::io::Error),
    /// Content diff error
    ContentDiff(crate::content_diff::Error<Store>),
    /// Unresolved merge conflicts
    UnresolvedConflicts(Vec<MergeConflict>),
    /// Other errors with message
    Other(String),
}

impl<Store: ObjectStore> From<std::io::Error> for Error<Store> {
    fn from(error: std::io::Error) -> Self {
        Error::IO(error)
    }
}

impl<Store: ObjectStore> From<serde_json::Error> for Error<Store> {
    fn from(error: serde_json::Error) -> Self {
        Error::ParseError(error)
    }
}

impl<Store: ObjectStore> From<crate::content_diff::Error<Store>> for Error<Store> {
    fn from(error: crate::content_diff::Error<Store>) -> Self {
        Error::ContentDiff(error)
    }
}

/// Represents a conflict between two branches during a merge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeConflict {
    /// Path to the conflicting file
    pub path: PathBuf,
    /// The base version of the file (common ancestor)
    pub base_id: Option<ObjectId>,
    /// The version from the current branch
    pub ours_id: Option<ObjectId>,
    /// The version from the branch being merged
    pub theirs_id: Option<ObjectId>,
    /// Conflict type
    pub conflict_type: ConflictType,
    /// Indicates if this conflict has been resolved
    pub resolved: bool,
    /// If resolved, the new content ID
    pub resolution_id: Option<ObjectId>,
}

/// Types of merge conflicts that can occur
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Both branches modified the same file
    BothModified,
    /// One branch modified the file, other deleted it
    ModifiedDeleted,
    /// Both branches added the file with different content
    BothAdded,
    /// One branch changed a file, the other changed its type (file/directory)
    TypeChanged,
}

impl fmt::Display for ConflictType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConflictType::BothModified => write!(f, "both modified"),
            ConflictType::ModifiedDeleted => write!(f, "modified in one branch, deleted in another"),
            ConflictType::BothAdded => write!(f, "added in both branches with different content"),
            ConflictType::TypeChanged => write!(f, "type changed (file/directory)"),
        }
    }
}

impl fmt::Display for MergeConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.resolved {
            "RESOLVED"
        } else {
            "UNRESOLVED"
        };
        
        write!(
            f,
            "{}: {} ({})",
            status,
            self.path.display(),
            self.conflict_type
        )
    }
}

/// Result of a merge operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    /// The base snapshot ID (common ancestor)
    pub base_id: ObjectId,
    /// The source snapshot ID (current branch)
    pub ours_id: ObjectId,
    /// The target snapshot ID (branch being merged)
    pub theirs_id: ObjectId,
    /// The resulting directory after the merge
    pub merged_directory: Option<Directory>,
    /// List of conflicts that occurred during the merge
    pub conflicts: Vec<MergeConflict>,
    /// Whether the merge was successful (no conflicts or all resolved)
    pub success: bool,
}

impl MergeResult {
    /// Checks if there are any unresolved conflicts
    pub fn has_unresolved_conflicts(&self) -> bool {
        self.conflicts.iter().any(|conflict| !conflict.resolved)
    }
    
    /// Creates a snapshot for a successful merge
    pub fn create_snapshot<Store>(
        &self,
        store: &mut Store,
        message: String,
    ) -> Result<ObjectId, Error<Store>>
    where
        Store: ObjectStore,
        Store: InsertJson
    {
        if self.has_unresolved_conflicts() {
            return Err(Error::UnresolvedConflicts(self.conflicts.clone()));
        }

        // Create a final merged directory by recreating it with resolved conflicts
        let mut final_directory = if let Some(merged_dir) = &self.merged_directory {
            merged_dir.clone()
        } else {
            // If no merged directory exists, we need to reconstruct one from the base
            // This should only happen in test scenarios
            let base_bytes = store.read(self.base_id).map_err(Error::Store)?
                .ok_or(Error::ObjectMissing(self.base_id))?;
            let base: SnapShot = serde_json::from_slice(&base_bytes)?;

            let base_dir_bytes = store.read(base.directory).map_err(Error::Store)?
                .ok_or(Error::ObjectMissing(base.directory))?;
            serde_json::from_slice(&base_dir_bytes)?
        };

        // Apply the resolved conflicts to the directory
        for conflict in &self.conflicts {
            if !conflict.resolved || conflict.resolution_id.is_none() {
                continue;
            }

            let path_parts: Vec<&str> = conflict.path.to_str()
                .ok_or_else(|| Error::Other(format!("Invalid path: {:?}", conflict.path)))?
                .split('/')
                .collect();

            // Navigate to the right location in the directory tree
            let mut current_dir = &mut final_directory;
            let mut current_path = vec![];

            // Navigate to the parent directory
            for i in 0..path_parts.len() - 1 {
                let part = path_parts[i];
                current_path.push(part);

                // We need to handle getting the directory in a way that doesn't
                // result in multiple mutable borrows
                let dir_exists = match current_dir.root.get(part) {
                    Some(DirectoryEntry::Directory(_)) => true,
                    _ => false
                };

                if !dir_exists {
                    // Create missing directory
                    let new_dir = Box::new(Directory { root: BTreeMap::new() });
                    current_dir.root.insert(part.to_string(), DirectoryEntry::Directory(new_dir));
                }

                // At this point we know there's a directory entry
                if let Some(DirectoryEntry::Directory(dir)) = current_dir.root.get_mut(part) {
                    current_dir = dir;
                } else {
                    return Err(Error::Other(format!("Failed to access directory: {}", part)));
                }
            }

            // Add the resolved file
            let file_name = path_parts.last().unwrap().to_string();
            current_dir.root.insert(file_name, DirectoryEntry::File(conflict.resolution_id.unwrap()));
        }

        // Save the merged directory
        let dir_id = match store.insert_json(&final_directory) {
            Ok(id) => id,
            Err(e) => return Err(Error::Other(format!("Failed to save directory: {}", e))),
        };

        // Create a snapshot with multiple parents
        let mut parents = BTreeSet::new();
        parents.insert(self.ours_id);
        parents.insert(self.theirs_id);

        let snapshot = SnapShot {
            message,
            directory: dir_id,
            previous: parents,
        };

        // Store and return the snapshot ID
        let snapshot_id = match store.insert_json(&snapshot) {
            Ok(id) => id,
            Err(e) => return Err(Error::Other(format!("Failed to save snapshot: {}", e))),
        };

        Ok(snapshot_id)
    }
}

/// Merge strategy for resolving conflicts automatically
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Normal mode - requires manual resolution
    Normal,
    /// Always use our version
    Ours,
    /// Always use their version
    Theirs,
}

impl MergeStrategy {
    /// Parse a string into a merge strategy
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "normal" => Ok(MergeStrategy::Normal),
            "ours" => Ok(MergeStrategy::Ours),
            "theirs" => Ok(MergeStrategy::Theirs),
            _ => Err(format!("Invalid merge strategy: {}. Valid options are 'normal', 'ours', or 'theirs'", s)),
        }
    }
}

/// Utility to help resolve a conflict
#[derive(Debug, Clone)]
pub struct ConflictResolver {
    /// Conflict being resolved
    pub conflict: MergeConflict,
    /// Base content
    pub base_content: Option<Vec<u8>>,
    /// Our version content
    pub ours_content: Option<Vec<u8>>,
    /// Their version content
    pub theirs_content: Option<Vec<u8>>,
    /// Content with markers for user to resolve
    pub marked_content: Vec<u8>,
}

impl ConflictResolver {
    /// Creates a conflict resolver for a specific conflict
    pub fn new<Store>(
        store: &Store,
        conflict: MergeConflict,
        our_branch: &str,
        their_branch: &str
    ) -> Result<Self, Error<Store>>
    where
        Store: ObjectStore
    {
        // Load content for each version if present
        let base_content = if let Some(id) = conflict.base_id {
            store.read(id).map_err(Error::Store)?.map(|v| v.to_vec())
        } else {
            None
        };

        let ours_content = if let Some(id) = conflict.ours_id {
            store.read(id).map_err(Error::Store)?.map(|v| v.to_vec())
        } else {
            None
        };

        let theirs_content = if let Some(id) = conflict.theirs_id {
            store.read(id).map_err(Error::Store)?.map(|v| v.to_vec())
        } else {
            None
        };

        // Create marked content with conflict markers
        let marked_content = Self::create_marked_content(
            &base_content,
            &ours_content,
            &theirs_content,
            our_branch,
            their_branch
        );

        Ok(ConflictResolver {
            conflict,
            base_content,
            ours_content,
            theirs_content,
            marked_content,
        })
    }

    /// Formats content with standard git-style diff3 conflict markers for
    /// manual resolution.
    ///
    /// When all three versions are present (the common both-modified case) the
    /// markers are produced by the same Myers/diff3 engine used for
    /// auto-merging, so only the genuinely conflicting hunks are marked and the
    /// output matches what git users expect. For add/add or modify/delete
    /// conflicts, where one side has no content, we fall back to a simple
    /// two-way marker block.
    fn create_marked_content(
        base: &Option<Vec<u8>>,
        ours: &Option<Vec<u8>>,
        theirs: &Option<Vec<u8>>,
        our_branch: &str,
        their_branch: &str,
    ) -> Vec<u8> {
        if let (Some(base_c), Some(ours_c), Some(theirs_c)) = (base, ours, theirs) {
            // Default MergeOptions uses ConflictStyle::Diff3 (shows the base).
            // Ok means diffy found no textual conflict; either way the returned
            // bytes are what the user should edit.
            let marked = match diffy::MergeOptions::new().merge_bytes(base_c, ours_c, theirs_c) {
                Ok(clean) => clean,
                Err(conflicted) => conflicted,
            };
            return relabel_conflict_markers(&marked, our_branch, their_branch);
        }

        // Fallback: one side is absent (add/add or modify/delete). Emit a plain
        // two-way block using the same marker vocabulary git uses.
        let mut result = Vec::new();
        result.extend_from_slice(format!("<<<<<<< {our_branch} (ours)\n").as_bytes());
        if let Some(content) = ours {
            result.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                result.push(b'\n');
            }
        }
        result.extend_from_slice(b"=======\n");
        if let Some(content) = theirs {
            result.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                result.push(b'\n');
            }
        }
        result.extend_from_slice(format!(">>>>>>> {their_branch} (theirs)\n").as_bytes());
        result
    }

    /// Resolves the conflict by saving the edited content
    pub fn resolve<Store>(
        &self,
        store: &mut Store,
        resolved_content: &[u8],
    ) -> Result<ObjectId, Error<Store>>
    where
        Store: ObjectStore
    {
        // Store the resolved content
        let id = store.insert(resolved_content).map_err(Error::Store)?;
        Ok(id)
    }

    /// Resolves the conflict using the specified merge strategy
    pub fn resolve_with_strategy<Store>(
        &self,
        store: &mut Store,
        strategy: MergeStrategy,
    ) -> Result<Option<ObjectId>, Error<Store>>
    where
        Store: ObjectStore
    {
        match strategy {
            MergeStrategy::Normal => {
                // Normal strategy requires manual resolution
                Ok(None)
            },
            MergeStrategy::Ours => {
                // Use our version if available
                if let Some(content) = &self.ours_content {
                    let id = store.insert(content).map_err(Error::Store)?;
                    Ok(Some(id))
                } else {
                    // Our version not available
                    Err(Error::Other(format!(
                        "Cannot resolve conflict using 'ours' strategy: our version not available for {}",
                        self.conflict.path.display()
                    )))
                }
            },
            MergeStrategy::Theirs => {
                // Use their version if available
                if let Some(content) = &self.theirs_content {
                    let id = store.insert(content).map_err(Error::Store)?;
                    Ok(Some(id))
                } else {
                    // Their version not available
                    Err(Error::Other(format!(
                        "Cannot resolve conflict using 'theirs' strategy: their version not available for {}",
                        self.conflict.path.display()
                    )))
                }
            }
        }
    }
}

/// Performs a three-way merge between two snapshots
pub fn merge<Store: ObjectStore>(
    store: &mut Store,
    base_id: ObjectId,    // Common ancestor
    ours_id: ObjectId,    // Current branch
    theirs_id: ObjectId,  // Branch being merged
    our_branch: &str,     // Current branch name
    their_branch: &str,   // Branch being merged name
    write_conflict_markers: bool, // Whether to write conflict markers to files
) -> Result<MergeResult, Error<Store>> {
    // Get the snapshots from the store
    let base_bytes = store.read(base_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(base_id))?;
    let ours_bytes = store.read(ours_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(ours_id))?;
    let theirs_bytes = store.read(theirs_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(theirs_id))?;

    // Deserialize the snapshots
    let base: SnapShot = serde_json::from_slice(&base_bytes)?;
    let ours: SnapShot = serde_json::from_slice(&ours_bytes)?;
    let theirs: SnapShot = serde_json::from_slice(&theirs_bytes)?;

    // Get the directory structures
    let base_dir_bytes = store.read(base.directory).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(base.directory))?;
    let ours_dir_bytes = store.read(ours.directory).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(ours.directory))?;
    let theirs_dir_bytes = store.read(theirs.directory).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(theirs.directory))?;

    // Deserialize the directories
    let base_dir: Directory = serde_json::from_slice(&base_dir_bytes)?;
    let ours_dir: Directory = serde_json::from_slice(&ours_dir_bytes)?;
    let theirs_dir: Directory = serde_json::from_slice(&theirs_dir_bytes)?;

    // Perform the merge
    let (merged_dir, conflicts) = three_way_merge(store, base_dir, ours_dir, theirs_dir)?;
    let has_conflicts = !conflicts.is_empty();

    // If there are conflicts and we should write markers, create files with conflict markers
    if has_conflicts && write_conflict_markers {
        for conflict in &conflicts {
            let file_path = &conflict.path;

            // Create a resolver for this conflict
            let resolver = ConflictResolver::new(store, conflict.clone(), our_branch, their_branch)?;

            // Write the conflict markers to the file
            if let Err(e) = std::fs::write(file_path, &resolver.marked_content) {
                println!("Warning: Failed to write conflict markers to {}: {}", file_path.display(), e);
            }
        }
    }

    // Always keep the merged tree, even when there are conflicts. It already
    // contains every non-conflicting change from both sides, with conflicted
    // paths left at their base version; create_snapshot then overlays the
    // resolved versions on top. Discarding it here (the old behaviour) caused
    // resolving one conflict to silently revert every other change in the merge.
    Ok(MergeResult {
        base_id,
        ours_id,
        theirs_id,
        merged_directory: Some(merged_dir),
        conflicts,
        success: !has_conflicts,
    })
}

/// Find the nearest common ancestor of two snapshots
pub fn find_common_ancestor<Store: ObjectStore>(
    store: &Store,
    snapshot1_id: ObjectId,
    snapshot2_id: ObjectId,
) -> Result<Option<ObjectId>, Error<Store>> {
    // If the snapshots are identical, they are their own common ancestor
    if snapshot1_id == snapshot2_id {
        return Ok(Some(snapshot1_id));
    }
    
    // Load the snapshots
    let snapshot1_bytes = store.read(snapshot1_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(snapshot1_id))?;
    let snapshot2_bytes = store.read(snapshot2_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(snapshot2_id))?;
    
    let snapshot1: SnapShot = serde_json::from_slice(&snapshot1_bytes)?;
    let snapshot2: SnapShot = serde_json::from_slice(&snapshot2_bytes)?;
    
    // If one snapshot is a direct parent of the other, that's the common ancestor
    if snapshot1.previous.contains(&snapshot2_id) {
        return Ok(Some(snapshot2_id));
    }
    if snapshot2.previous.contains(&snapshot1_id) {
        return Ok(Some(snapshot1_id));
    }
    
    // The merge base is a *lowest* common ancestor: a snapshot that is an
    // ancestor of both tips and is not itself an ancestor of any other common
    // ancestor. The previous code ranked common ancestors by summed hop
    // distance, which can pick a strict ancestor of the true base and
    // manufacture spurious conflicts.
    let ancestors1 = ancestors_including_self(store, snapshot1_id)?;
    let ancestors2 = ancestors_including_self(store, snapshot2_id)?;

    let common: BTreeSet<ObjectId> =
        ancestors1.intersection(&ancestors2).copied().collect();
    if common.is_empty() {
        // Unrelated histories.
        return Ok(None);
    }

    // Any common ancestor that is a *proper* ancestor of another common
    // ancestor is not a lowest common ancestor. Whatever remains are the merge
    // base candidates.
    let mut superseded: BTreeSet<ObjectId> = BTreeSet::new();
    for &c in &common {
        for parent in proper_ancestors(store, c)? {
            if common.contains(&parent) {
                superseded.insert(parent);
            }
        }
    }
    let mut bases: Vec<ObjectId> =
        common.iter().copied().filter(|c| !superseded.contains(c)).collect();

    // With a single base (the common case) we are done. Criss-cross histories
    // can leave several equally-valid bases; a true recursive merge would merge
    // them, but for now we pick one deterministically — the one with the most
    // ancestors (closest to the tips), breaking ties by id so the result never
    // depends on hashing order.
    bases.sort_by(|a, b| {
        let da = ancestor_count(store, *a).unwrap_or(0);
        let db = ancestor_count(store, *b).unwrap_or(0);
        db.cmp(&da).then_with(|| a.cmp(b))
    });

    Ok(bases.into_iter().next())
}

/// All ancestors of `snapshot_id`, including the snapshot itself. A node is
/// considered its own ancestor so that "one tip is an ancestor of the other"
/// falls out of the common-ancestor computation naturally.
fn ancestors_including_self<Store: ObjectStore>(
    store: &Store,
    snapshot_id: ObjectId,
) -> Result<BTreeSet<ObjectId>, Error<Store>> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![snapshot_id];
    while let Some(current_id) = stack.pop() {
        if !seen.insert(current_id) {
            continue;
        }
        let bytes = match store.read(current_id).map_err(Error::Store)? {
            Some(bytes) => bytes,
            None => continue,
        };
        let snapshot: SnapShot = match serde_json::from_slice(&bytes) {
            Ok(snap) => snap,
            Err(_) => continue,
        };
        for parent_id in snapshot.previous {
            if !seen.contains(&parent_id) {
                stack.push(parent_id);
            }
        }
    }
    Ok(seen)
}

/// Ancestors of `snapshot_id` excluding the snapshot itself.
fn proper_ancestors<Store: ObjectStore>(
    store: &Store,
    snapshot_id: ObjectId,
) -> Result<BTreeSet<ObjectId>, Error<Store>> {
    let mut set = ancestors_including_self(store, snapshot_id)?;
    set.remove(&snapshot_id);
    Ok(set)
}

/// Number of proper ancestors of `snapshot_id`, used only as a deterministic
/// tie-breaker between equally-valid merge bases.
fn ancestor_count<Store: ObjectStore>(
    store: &Store,
    snapshot_id: ObjectId,
) -> Result<usize, Error<Store>> {
    Ok(proper_ancestors(store, snapshot_id)?.len())
}

/// Perform a three-way merge between directories
/// Returns the merged directory and any conflicts that occurred
fn three_way_merge<Store: ObjectStore>(
    store: &mut Store,
    base: Directory,
    ours: Directory,
    theirs: Directory,
) -> Result<(Directory, Vec<MergeConflict>), Error<Store>> {
    // Create diffs from base to each branch
    let ours_diff = base.diff(&ours);
    let theirs_diff = base.diff(&theirs);
    
    // Start with a copy of the base directory
    let mut merged = base.clone();
    let mut conflicts = Vec::new();
    
    // Process both diffs to create a merged directory
    merge_diffs(
        store,
        &mut merged,
        &ours_diff,
        &theirs_diff,
        &mut conflicts,
        PathBuf::new(),
    )?;
    
    Ok((merged, conflicts))
}

/// Merge two diffs into a single directory
fn merge_diffs<Store: ObjectStore>(
    store: &mut Store,
    merged: &mut Directory,
    ours_diff: &Diff,
    theirs_diff: &Diff,
    conflicts: &mut Vec<MergeConflict>,
    path_prefix: PathBuf,
) -> Result<(), Error<Store>> {
    // Process files deleted in both branches
    for path in ours_diff.deleted.iter().filter(|p| theirs_diff.deleted.contains(*p)) {
        // If both deleted the same file, remove it from merged result
        merged.root.remove(path);
    }
    
    // Process files deleted in our branch only
    for path in ours_diff.deleted.iter().filter(|p| !theirs_diff.deleted.contains(*p)) {
        // Check if theirs modified it (conflict)
        if let Some(theirs_entry) = theirs_diff.modified.get(path) {
            // Conflict: we deleted, they modified
            let conflict_path = path_prefix.join(path);
            let base_id = if let Some(base_entry) = merged.root.get(path) {
                match base_entry {
                    DirectoryEntry::File(id) => Some(*id),
                    _ => None, // Directory case is handled separately
                }
            } else {
                None
            };
            
            let theirs_id = match theirs_entry {
                crate::directory::DiffEntry::File(id) => Some(*id),
                crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => Some(*new_id),
                _ => None, // Directory case is handled separately
            };
            
            conflicts.push(MergeConflict {
                path: conflict_path,
                base_id,
                ours_id: None, // We deleted it
                theirs_id,
                conflict_type: ConflictType::ModifiedDeleted,
                resolved: false,
                resolution_id: None,
            });
        } else if !theirs_diff.added.contains_key(path) {
            // They didn't modify or add it, so we can safely delete it
            merged.root.remove(path);
        }
    }
    
    // Process files deleted in their branch only
    for path in theirs_diff.deleted.iter().filter(|p| !ours_diff.deleted.contains(*p)) {
        // Check if we modified it (conflict)
        if let Some(ours_entry) = ours_diff.modified.get(path) {
            // Conflict: they deleted, we modified
            let conflict_path = path_prefix.join(path);
            let base_id = if let Some(base_entry) = merged.root.get(path) {
                match base_entry {
                    DirectoryEntry::File(id) => Some(*id),
                    _ => None, // Directory case is handled separately
                }
            } else {
                None
            };
            
            let ours_id = match ours_entry {
                crate::directory::DiffEntry::File(id) => Some(*id),
                crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => Some(*new_id),
                _ => None, // Directory case is handled separately
            };
            
            conflicts.push(MergeConflict {
                path: conflict_path,
                base_id,
                ours_id,
                theirs_id: None, // They deleted it
                conflict_type: ConflictType::ModifiedDeleted,
                resolved: false,
                resolution_id: None,
            });
        } else if !ours_diff.added.contains_key(path) {
            // We didn't modify or add it, so we can safely delete it
            merged.root.remove(path);
        }
    }
    
    // Process files added in both branches
    for path in ours_diff.added.keys().filter(|p| theirs_diff.added.contains_key(*p)) {
        let ours_entry = ours_diff.added.get(path).unwrap();
        let theirs_entry = theirs_diff.added.get(path).unwrap();
        
        match (ours_entry, theirs_entry) {
            (DirectoryEntry::File(ours_id), DirectoryEntry::File(theirs_id)) => {
                if ours_id == theirs_id {
                    // Both added the same file, no conflict
                    merged.root.insert(path.clone(), DirectoryEntry::File(*ours_id));
                } else {
                    // Conflict: both added different versions
                    let conflict_path = path_prefix.join(path);
                    conflicts.push(MergeConflict {
                        path: conflict_path,
                        base_id: None, // It didn't exist in base
                        ours_id: Some(*ours_id),
                        theirs_id: Some(*theirs_id),
                        conflict_type: ConflictType::BothAdded,
                        resolved: false,
                        resolution_id: None,
                    });
                }
            },
            (DirectoryEntry::Directory(ours_dir), DirectoryEntry::Directory(theirs_dir)) => {
                // Both added directories, merge their contents recursively
                let mut new_dir = Directory::default();
                let new_path_prefix = path_prefix.join(path);
                
                // Create diffs for the subdirectories
                let ours_subdir_diff = Directory::default().diff(ours_dir);
                let theirs_subdir_diff = Directory::default().diff(theirs_dir);
                
                // Recursively merge the subdirectories
                merge_diffs(
                    store,
                    &mut new_dir,
                    &ours_subdir_diff,
                    &theirs_subdir_diff,
                    conflicts,
                    new_path_prefix,
                )?;
                
                // Add the merged directory
                merged.root.insert(path.clone(), DirectoryEntry::Directory(Box::new(new_dir)));
            },
            _ => {
                // One side added a file, the other added a directory with the same name
                let conflict_path = path_prefix.join(path);
                conflicts.push(MergeConflict {
                    path: conflict_path,
                    base_id: None, // It didn't exist in base
                    ours_id: match ours_entry {
                        DirectoryEntry::File(id) => Some(*id),
                        _ => None,
                    },
                    theirs_id: match theirs_entry {
                        DirectoryEntry::File(id) => Some(*id),
                        _ => None,
                    },
                    conflict_type: ConflictType::TypeChanged,
                    resolved: false,
                    resolution_id: None,
                });
            }
        }
    }
    
    // Process files added only in our branch
    for (path, entry) in ours_diff.added.iter().filter(|(p, _)| !theirs_diff.added.contains_key(*p)) {
        // No conflict, just add it
        merged.root.insert(path.clone(), entry.clone());
    }
    
    // Process files added only in their branch
    for (path, entry) in theirs_diff.added.iter().filter(|(p, _)| !ours_diff.added.contains_key(*p)) {
        // No conflict, just add it
        merged.root.insert(path.clone(), entry.clone());
    }
    
    // Process files modified in both branches
    for path in ours_diff.modified.keys().filter(|p| theirs_diff.modified.contains_key(*p)) {
        let ours_entry = ours_diff.modified.get(path).unwrap();
        let theirs_entry = theirs_diff.modified.get(path).unwrap();
        
        match (ours_entry, theirs_entry) {
            (
                crate::directory::DiffEntry::File(ours_id) | 
                crate::directory::DiffEntry::FileWithContentDiff { new_id: ours_id, .. },
                crate::directory::DiffEntry::File(theirs_id) | 
                crate::directory::DiffEntry::FileWithContentDiff { new_id: theirs_id, .. }
            ) => {
                if ours_id == theirs_id {
                    // Both made identical changes, no conflict
                    merged.root.insert(path.clone(), DirectoryEntry::File(*ours_id));
                } else {
                    // Attempt to auto-merge the files
                    let base_id = if let Some(DirectoryEntry::File(id)) = merged.root.get(path) {
                        *id
                    } else {
                        // The base had a directory here that both sides replaced
                        // with (different) files. That is a type change, not
                        // something we can three-way merge, so record it as a
                        // conflict rather than aborting the whole merge with a
                        // fabricated "object missing" error.
                        conflicts.push(MergeConflict {
                            path: path_prefix.join(path),
                            base_id: None,
                            ours_id: Some(*ours_id),
                            theirs_id: Some(*theirs_id),
                            conflict_type: ConflictType::TypeChanged,
                            resolved: false,
                            resolution_id: None,
                        });
                        continue;
                    };

                    match auto_merge_files(store, base_id, *ours_id, *theirs_id)? {
                        Some(merged_id) => {
                            // Auto-merge successful
                            merged.root.insert(path.clone(), DirectoryEntry::File(merged_id));
                        },
                        None => {
                            // Auto-merge failed, create a conflict
                            let conflict_path = path_prefix.join(path);
                            conflicts.push(MergeConflict {
                                path: conflict_path,
                                base_id: Some(base_id),
                                ours_id: Some(*ours_id),
                                theirs_id: Some(*theirs_id),
                                conflict_type: ConflictType::BothModified,
                                resolved: false,
                                resolution_id: None,
                            });
                        }
                    }
                }
            },
            (crate::directory::DiffEntry::Directory(ours_dir), crate::directory::DiffEntry::Directory(theirs_dir)) => {
                // Both modified directories, merge their contents recursively
                let base_dir = if let Some(DirectoryEntry::Directory(dir)) = merged.root.get(path) {
                    dir.clone()
                } else {
                    // This shouldn't happen in theory, but handle it just in case
                    Box::new(Directory::default())
                };
                
                let mut new_dir = base_dir.clone();
                let new_path_prefix = path_prefix.join(path);
                
                // Recursively merge the directories
                merge_diffs(
                    store,
                    &mut new_dir,
                    ours_dir,
                    theirs_dir,
                    conflicts,
                    new_path_prefix,
                )?;
                
                // Update the merged directory
                merged.root.insert(path.clone(), DirectoryEntry::Directory(Box::new(*new_dir)));
            },
            _ => {
                // One modified a file, the other changed it to a directory or vice versa
                let conflict_path = path_prefix.join(path);
                let base_id = if let Some(base_entry) = merged.root.get(path) {
                    match base_entry {
                        DirectoryEntry::File(id) => Some(*id),
                        _ => None,
                    }
                } else {
                    None
                };
                
                let ours_id = match ours_entry {
                    crate::directory::DiffEntry::File(id) => Some(*id),
                    crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => Some(*new_id),
                    _ => None,
                };
                
                let theirs_id = match theirs_entry {
                    crate::directory::DiffEntry::File(id) => Some(*id),
                    crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => Some(*new_id),
                    _ => None,
                };
                
                conflicts.push(MergeConflict {
                    path: conflict_path,
                    base_id,
                    ours_id,
                    theirs_id,
                    conflict_type: ConflictType::TypeChanged,
                    resolved: false,
                    resolution_id: None,
                });
            }
        }
    }
    
    // Process files modified only in our branch
    for (path, entry) in ours_diff.modified.iter().filter(|(p, _)| !theirs_diff.modified.contains_key(*p)) {
        match entry {
            crate::directory::DiffEntry::File(id) => {
                merged.root.insert(path.clone(), DirectoryEntry::File(*id));
            },
            crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => {
                merged.root.insert(path.clone(), DirectoryEntry::File(*new_id));
            },
            crate::directory::DiffEntry::Directory(dir) => {
                // Get the base directory
                let base_dir = if let Some(DirectoryEntry::Directory(dir)) = merged.root.get(path) {
                    dir.clone()
                } else {
                    // This shouldn't happen in theory, but handle it just in case
                    Box::new(Directory::default())
                };
                
                // Apply our changes to the base directory
                let mut new_dir = base_dir.clone();
                apply_diff(&mut new_dir, dir);
                
                // Update the merged directory
                merged.root.insert(path.clone(), DirectoryEntry::Directory(Box::new(*new_dir)));
            }
        }
    }
    
    // Process files modified only in their branch
    for (path, entry) in theirs_diff.modified.iter().filter(|(p, _)| !ours_diff.modified.contains_key(*p)) {
        match entry {
            crate::directory::DiffEntry::File(id) => {
                merged.root.insert(path.clone(), DirectoryEntry::File(*id));
            },
            crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => {
                merged.root.insert(path.clone(), DirectoryEntry::File(*new_id));
            },
            crate::directory::DiffEntry::Directory(dir) => {
                // Get the base directory
                let base_dir = if let Some(DirectoryEntry::Directory(dir)) = merged.root.get(path) {
                    dir.clone()
                } else {
                    // This shouldn't happen in theory, but handle it just in case
                    Box::new(Directory::default())
                };
                
                // Apply their changes to the base directory
                let mut new_dir = base_dir.clone();
                apply_diff(&mut new_dir, dir);
                
                // Update the merged directory
                merged.root.insert(path.clone(), DirectoryEntry::Directory(Box::new(*new_dir)));
            }
        }
    }
    
    Ok(())
}

/// Apply a diff to a directory
fn apply_diff(directory: &mut Directory, diff: &Diff) {
    // Remove deleted files
    for path in &diff.deleted {
        directory.root.remove(path);
    }
    
    // Add new files
    for (path, entry) in &diff.added {
        directory.root.insert(path.clone(), entry.clone());
    }
    
    // Apply modifications
    for (path, entry) in &diff.modified {
        match entry {
            crate::directory::DiffEntry::File(id) => {
                directory.root.insert(path.clone(), DirectoryEntry::File(*id));
            },
            crate::directory::DiffEntry::FileWithContentDiff { new_id, .. } => {
                directory.root.insert(path.clone(), DirectoryEntry::File(*new_id));
            },
            crate::directory::DiffEntry::Directory(dir) => {
                // Get the existing directory
                if let Some(DirectoryEntry::Directory(existing_dir)) = directory.root.get_mut(path) {
                    // Apply the diff recursively
                    apply_diff(existing_dir, dir);
                } else {
                    // Replace with the new directory
                    directory.root.insert(
                        path.clone(),
                        DirectoryEntry::Directory(Box::new(Directory {
                            root: BTreeMap::new(),
                        })),
                    );
                    
                    if let Some(DirectoryEntry::Directory(dir_entry)) = directory.root.get_mut(path) {
                        apply_diff(dir_entry, dir);
                    }
                }
            }
        }
    }
}

/// Attempts to automatically merge two files given their common base.
///
/// Returns `Some(id)` for a clean merge, or `None` if the two sides made
/// conflicting changes and manual resolution is required. Delegates the
/// line-level work to `diffy`, which uses Myers diffing and produces the same
/// results git users expect. Because it operates on raw bytes, a clean merge
/// preserves the file exactly: CRLF line endings, the presence or absence of a
/// trailing newline, and non-UTF-8 (binary) content all survive unchanged.
fn auto_merge_files<Store: ObjectStore>(
    store: &mut Store,
    base_id: ObjectId,
    ours_id: ObjectId,
    theirs_id: ObjectId,
) -> Result<Option<ObjectId>, Error<Store>> {
    // Resolve the trivial cases by object identity. These are exact for any
    // content, binary included, and avoid loading bytes we do not need.
    if ours_id == theirs_id {
        return Ok(Some(ours_id));
    }
    if base_id == ours_id {
        // Only their side changed, so take theirs.
        return Ok(Some(theirs_id));
    }
    if base_id == theirs_id {
        // Only our side changed, so take ours.
        return Ok(Some(ours_id));
    }

    let base = store
        .read(base_id)
        .map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(base_id))?;
    let ours = store
        .read(ours_id)
        .map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(ours_id))?;
    let theirs = store
        .read(theirs_id)
        .map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(theirs_id))?;

    match diffy::merge_bytes(&base, &ours, &theirs) {
        Ok(merged) => {
            let merged_id = store.insert(&merged).map_err(Error::Store)?;
            Ok(Some(merged_id))
        }
        // Err carries the marker-annotated content; here we only need to know a
        // conflict occurred. The marked content for the working tree is
        // generated by ConflictResolver so branch names can be shown.
        Err(_conflicted) => Ok(None),
    }
}

/// Rewrites diffy's default conflict-marker labels to show branch names.
///
/// diffy emits `<<<<<<< ours`, `||||||| original`, `=======`, and
/// `>>>>>>> theirs`. We keep the standard marker glyphs (so any git-aware
/// editor or `merge --continue` recognises them) but relabel the endpoints with
/// the actual branch names, matching revtool's older, friendlier output.
fn relabel_conflict_markers(content: &[u8], our_branch: &str, their_branch: &str) -> Vec<u8> {
    let ours_label = format!("<<<<<<< {our_branch} (ours)\n").into_bytes();
    let base_label = b"||||||| base (common ancestor)\n".to_vec();
    let theirs_label = format!(">>>>>>> {their_branch} (theirs)\n").into_bytes();

    let mut out = Vec::with_capacity(content.len());
    for line in split_lines_keeping_ends(content) {
        if line_is_marker(line, b'<') {
            out.extend_from_slice(&ours_label);
        } else if line_is_marker(line, b'|') {
            out.extend_from_slice(&base_label);
        } else if line_is_marker(line, b'>') {
            out.extend_from_slice(&theirs_label);
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

/// True if `line` is a conflict marker line for glyph `marker` — a run of at
/// least seven `marker` bytes optionally followed by a space and label.
fn line_is_marker(line: &[u8], marker: u8) -> bool {
    let run = line.iter().take_while(|&&b| b == marker).count();
    run >= 7 && line.get(run).is_none_or(|&b| b == b' ' || b == b'\n' || b == b'\r')
}

/// Splits bytes into lines, keeping each line's terminator attached so the
/// pieces concatenate back into the original bytes exactly.
fn split_lines_keeping_ends(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            lines.push(&content[start..=i]);
            start = i + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_store::in_memory::InMemoryObjectStore;

    // Helper function to create test snapshots - recursive approach
    fn create_test_snapshot(
        store: &mut InMemoryObjectStore,
        parent_ids: Vec<ObjectId>,
        files: Vec<(&str, &[u8])>,
    ) -> ObjectId {
        // Helper function to add a file to a directory structure
        fn add_file_to_directory(dir: &mut Directory, path_parts: &[&str], file_id: ObjectId) {
            if path_parts.len() == 1 {
                // This is a file in the current directory
                dir.root.insert(path_parts[0].to_string(), DirectoryEntry::File(file_id));
            } else {
                // We need to go deeper into subdirectories
                let dir_name = path_parts[0].to_string();

                // Ensure the subdirectory exists
                if !dir.root.contains_key(&dir_name) {
                    // Create a new empty directory
                    dir.root.insert(dir_name.clone(), DirectoryEntry::Directory(Box::new(Directory {
                        root: BTreeMap::new()
                    })));
                }

                // Get the subdirectory and recurse
                if let Some(DirectoryEntry::Directory(subdir)) = dir.root.get_mut(&dir_name) {
                    add_file_to_directory(subdir, &path_parts[1..], file_id);
                }
            }
        }

        // Create root directory
        let mut directory = Directory { root: BTreeMap::new() };

        // Process each file, creating directories as needed
        for (path_str, content) in files {
            // Get file ID
            let file_id = store.insert(content).unwrap();

            // Split path into components
            let path_parts: Vec<&str> = path_str.split('/').collect();

            // Add file to the directory structure
            add_file_to_directory(&mut directory, &path_parts, file_id);
        }

        // Serialize and store the directory
        let dir_bytes = serde_json::to_vec(&directory).unwrap();
        let dir_id = store.insert(&dir_bytes).unwrap();

        // Create the snapshot
        let snapshot = SnapShot {
            message: "Test snapshot".to_string(),
            directory: dir_id,
            previous: parent_ids.into_iter().collect(),
        };

        let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
        store.insert(&snapshot_bytes).unwrap()
    }

    #[test]
    fn test_merge_strategy_parsing() {
        // Test valid strategies
        assert_eq!(MergeStrategy::from_str("normal").unwrap(), MergeStrategy::Normal);
        assert_eq!(MergeStrategy::from_str("ours").unwrap(), MergeStrategy::Ours);
        assert_eq!(MergeStrategy::from_str("theirs").unwrap(), MergeStrategy::Theirs);

        // Test case insensitivity
        assert_eq!(MergeStrategy::from_str("NORMAL").unwrap(), MergeStrategy::Normal);
        assert_eq!(MergeStrategy::from_str("Ours").unwrap(), MergeStrategy::Ours);
        assert_eq!(MergeStrategy::from_str("ThEiRs").unwrap(), MergeStrategy::Theirs);

        // Test invalid strategy
        assert!(MergeStrategy::from_str("invalid").is_err());
    }

    #[test]
    fn test_conflict_resolver_with_strategy() {
        let mut store = InMemoryObjectStore::new();

        // Create file contents
        let base_content = b"This is the base content";
        let ours_content = b"This is our modified content";
        let theirs_content = b"This is their modified content";

        // Store file contents
        let base_id = store.insert(base_content).unwrap();
        let ours_id = store.insert(ours_content).unwrap();
        let theirs_id = store.insert(theirs_content).unwrap();

        // Create a test conflict
        let conflict = MergeConflict {
            path: std::path::PathBuf::from("test_file.txt"),
            base_id: Some(base_id),
            ours_id: Some(ours_id),
            theirs_id: Some(theirs_id),
            conflict_type: ConflictType::BothModified,
            resolved: false,
            resolution_id: None,
        };

        // Create resolver
        let resolver = ConflictResolver::new(&store, conflict, "ours-branch", "theirs-branch").unwrap();

        // Test 'ours' strategy
        let ours_result = resolver.resolve_with_strategy(&mut store, MergeStrategy::Ours).unwrap();
        assert!(ours_result.is_some());
        let ours_resolved_id = ours_result.unwrap();
        let ours_resolved_content = store.read(ours_resolved_id).unwrap().unwrap();
        assert_eq!(ours_resolved_content, ours_content);

        // Test 'theirs' strategy
        let theirs_result = resolver.resolve_with_strategy(&mut store, MergeStrategy::Theirs).unwrap();
        assert!(theirs_result.is_some());
        let theirs_resolved_id = theirs_result.unwrap();
        let theirs_resolved_content = store.read(theirs_resolved_id).unwrap().unwrap();
        assert_eq!(theirs_resolved_content, theirs_content);

        // Test 'normal' strategy
        let normal_result = resolver.resolve_with_strategy(&mut store, MergeStrategy::Normal).unwrap();
        assert!(normal_result.is_none()); // Normal strategy requires manual resolution
    }

    #[test]
    fn test_conflict_markers_format() {
        let mut store = InMemoryObjectStore::new();

        // Create file contents
        let base_content = b"This is the base content\n";
        let ours_content = b"This is our modified content\n";
        let theirs_content = b"This is their modified content\n";

        // Store file contents
        let base_id = store.insert(base_content).unwrap();
        let ours_id = store.insert(ours_content).unwrap();
        let theirs_id = store.insert(theirs_content).unwrap();

        // Create a test conflict
        let conflict = MergeConflict {
            path: std::path::PathBuf::from("test_file.txt"),
            base_id: Some(base_id),
            ours_id: Some(ours_id),
            theirs_id: Some(theirs_id),
            conflict_type: ConflictType::BothModified,
            resolved: false,
            resolution_id: None,
        };

        // Create resolver
        let resolver = ConflictResolver::new(&store, conflict, "main", "feature").unwrap();

        // Check conflict markers in the marked content
        let marked_content = String::from_utf8_lossy(&resolver.marked_content);

        // Assert standard git-style diff3 markers are present, relabelled with
        // the branch names.
        assert!(marked_content.contains("<<<<<<< main (ours)"), "{marked_content}");
        assert!(marked_content.contains("======="), "{marked_content}");
        assert!(marked_content.contains(">>>>>>> feature (theirs)"), "{marked_content}");
        assert!(marked_content.contains("||||||| base (common ancestor)"), "{marked_content}");

        // Markers must appear in valid diff3 order: ours, then base, then
        // separator, then theirs. The old code emitted the base section after
        // the closing >>>>>>> marker, which no diff3 tool can parse.
        let ours_pos = marked_content.find("<<<<<<<").unwrap();
        let base_pos = marked_content.find("|||||||").unwrap();
        let sep_pos = marked_content.find("\n=======").unwrap();
        let theirs_pos = marked_content.find(">>>>>>>").unwrap();
        assert!(
            ours_pos < base_pos && base_pos < sep_pos && sep_pos < theirs_pos,
            "markers out of order: {marked_content}"
        );

        // Check that all content versions are included
        assert!(marked_content.contains("This is the base content"));
        assert!(marked_content.contains("This is our modified content"));
        assert!(marked_content.contains("This is their modified content"));
    }

    #[test]
    fn test_conflict_resolver_with_strategy_missing_content() {
        let mut store = InMemoryObjectStore::new();

        // Create file contents for only one side
        let base_content = b"This is the base content";
        let ours_content = b"This is our modified content";

        // Store file contents
        let base_id = store.insert(base_content).unwrap();
        let ours_id = store.insert(ours_content).unwrap();

        // Create a test conflict (with no 'theirs' content)
        let conflict = MergeConflict {
            path: std::path::PathBuf::from("test_file.txt"),
            base_id: Some(base_id),
            ours_id: Some(ours_id),
            theirs_id: None,
            conflict_type: ConflictType::ModifiedDeleted,
            resolved: false,
            resolution_id: None,
        };

        // Create resolver
        let resolver = ConflictResolver::new(&store, conflict, "ours-branch", "theirs-branch").unwrap();

        // Test 'ours' strategy - should work
        let ours_result = resolver.resolve_with_strategy(&mut store, MergeStrategy::Ours).unwrap();
        assert!(ours_result.is_some());

        // Test 'theirs' strategy - should fail
        let theirs_result = resolver.resolve_with_strategy(&mut store, MergeStrategy::Theirs);
        assert!(theirs_result.is_err());
    }
    
    #[test]
    fn test_find_common_ancestor() {
        let mut store = InMemoryObjectStore::new();

        // Create a simple history:
        // A <- B <- C
        //      ↖
        //        D <- E

        // Create base snapshot A (no parent)
        let a_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![("file1.txt", b"base content")]
        );

        // Create snapshot B (child of A)
        let b_id = create_test_snapshot(
            &mut store,
            vec![a_id],
            vec![
                ("file1.txt", b"modified in B"),
                ("file2.txt", b"added in B")
            ]
        );

        // Create snapshot C (child of B)
        let c_id = create_test_snapshot(
            &mut store,
            vec![b_id],
            vec![
                ("file1.txt", b"modified in C"),
                ("file2.txt", b"modified in C")
            ]
        );

        // Create snapshot D (child of B)
        let d_id = create_test_snapshot(
            &mut store,
            vec![b_id],
            vec![
                ("file1.txt", b"modified in D"),
                ("file3.txt", b"added in D")
            ]
        );

        // Create snapshot E (child of D)
        let e_id = create_test_snapshot(
            &mut store,
            vec![d_id],
            vec![
                ("file1.txt", b"modified in E"),
                ("file3.txt", b"modified in E")
            ]
        );

        // Test finding common ancestors

        // C and E should have B as their common ancestor
        let ancestor = find_common_ancestor(&store, c_id, e_id).unwrap();
        assert_eq!(ancestor, Some(b_id));

        // C and D should have B as their common ancestor
        let ancestor = find_common_ancestor(&store, c_id, d_id).unwrap();
        assert_eq!(ancestor, Some(b_id));

        // B and E should have B as their common ancestor
        let ancestor = find_common_ancestor(&store, b_id, e_id).unwrap();
        assert_eq!(ancestor, Some(b_id));

        // A and E should have A as their common ancestor
        let ancestor = find_common_ancestor(&store, a_id, e_id).unwrap();
        assert_eq!(ancestor, Some(a_id));
    }

    #[test]
    fn test_find_most_recent_common_ancestor() {
        let mut store = InMemoryObjectStore::new();

        // Create a more complex history to test the "most recent" logic:
        //
        // A <- B <- C <- D <- E
        //      ↖         ↖
        //        F <- G    H <- I

        // A is the root
        let a_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![("file1.txt", b"base content")]
        );

        // Linear progression: B, C, D, E
        let b_id = create_test_snapshot(&mut store, vec![a_id], vec![("file1.txt", b"B")]);
        let c_id = create_test_snapshot(&mut store, vec![b_id], vec![("file1.txt", b"C")]);
        let d_id = create_test_snapshot(&mut store, vec![c_id], vec![("file1.txt", b"D")]);
        let e_id = create_test_snapshot(&mut store, vec![d_id], vec![("file1.txt", b"E")]);

        // Branch from B: F, G
        let f_id = create_test_snapshot(&mut store, vec![b_id], vec![("file1.txt", b"F")]);
        let g_id = create_test_snapshot(&mut store, vec![f_id], vec![("file1.txt", b"G")]);

        // Branch from D: H, I
        let h_id = create_test_snapshot(&mut store, vec![d_id], vec![("file1.txt", b"H")]);
        let i_id = create_test_snapshot(&mut store, vec![h_id], vec![("file1.txt", b"I")]);

        // Test cases for most recent common ancestor

        // Between E and G, the most recent common ancestor should be B
        let ancestor = find_common_ancestor(&store, e_id, g_id).unwrap();
        assert_eq!(ancestor, Some(b_id), "The MRCA of E and G should be B");

        // Between E and I, the most recent common ancestor should be D
        let ancestor = find_common_ancestor(&store, e_id, i_id).unwrap();
        assert_eq!(ancestor, Some(d_id), "The MRCA of E and I should be D");

        // Between G and I, the most recent common ancestor should be B
        let ancestor = find_common_ancestor(&store, g_id, i_id).unwrap();
        assert_eq!(ancestor, Some(b_id), "The MRCA of G and I should be B");

        // Create a merge scenario: J is a merge of G and I
        let j_id = create_test_snapshot(
            &mut store,
            vec![g_id, i_id],  // Multiple parents
            vec![("file1.txt", b"J")]
        );

        // Between J and E, the most recent common ancestor should be D (from I's history)
        let ancestor = find_common_ancestor(&store, j_id, e_id).unwrap();
        assert_eq!(ancestor, Some(d_id), "The MRCA of J and E should be D (via I)");
    }

    #[test]
    fn test_merge_base_prefers_ancestry_over_hop_distance() {
        // Regression: the old summed-distance heuristic returned a strict
        // ancestor of the true merge base here, producing spurious conflicts.
        //
        //   Q ── P ── L1 ── L2 ── L3 ──┐
        //   │    │                     ├── O   (merge of L3 and S)
        //   │    └── T                 │
        //   └── S ─────────────────────┘
        //
        // The lowest common ancestor of O and T is P, not Q: P is an ancestor
        // of both, and Q is only a common ancestor because it is P's parent.
        let mut store = InMemoryObjectStore::new();

        let q = create_test_snapshot(&mut store, vec![], vec![("f", b"Q")]);
        let p = create_test_snapshot(&mut store, vec![q], vec![("f", b"P")]);
        let l1 = create_test_snapshot(&mut store, vec![p], vec![("f", b"L1")]);
        let l2 = create_test_snapshot(&mut store, vec![l1], vec![("f", b"L2")]);
        let l3 = create_test_snapshot(&mut store, vec![l2], vec![("f", b"L3")]);
        let s = create_test_snapshot(&mut store, vec![q], vec![("f", b"S")]);
        let o = create_test_snapshot(&mut store, vec![l3, s], vec![("f", b"O")]);
        let t = create_test_snapshot(&mut store, vec![p], vec![("f", b"T")]);

        let base = find_common_ancestor(&store, o, t).unwrap();
        assert_eq!(base, Some(p), "merge base of O and T must be P, not Q");
    }

    #[test]
    fn test_merge_base_unrelated_histories_is_none() {
        let mut store = InMemoryObjectStore::new();
        let a = create_test_snapshot(&mut store, vec![], vec![("f", b"a")]);
        let b = create_test_snapshot(&mut store, vec![], vec![("g", b"b")]);
        assert_eq!(find_common_ancestor(&store, a, b).unwrap(), None);
    }

    #[test]
    fn test_auto_merge_no_conflict() {
        let mut store = InMemoryObjectStore::new();
        
        // Base version
        let base_content = b"Line 1\nLine 2\nLine 3\nLine 4\n";
        let base_id = store.insert(base_content).unwrap();
        
        // Our version (modified line 2)
        let ours_content = b"Line 1\nOur Line 2\nLine 3\nLine 4\n";
        let ours_id = store.insert(ours_content).unwrap();
        
        // Their version (modified line 4)
        let theirs_content = b"Line 1\nLine 2\nLine 3\nTheir Line 4\n";
        let theirs_id = store.insert(theirs_content).unwrap();
        
        // Auto-merge should succeed
        let result = auto_merge_files(&mut store, base_id, ours_id, theirs_id).unwrap();
        assert!(result.is_some());
        
        // Check the merged content
        let merged_id = result.unwrap();
        let merged_content = store.read(merged_id).unwrap().unwrap();
        let merged_str = String::from_utf8_lossy(&merged_content);
        
        // The merge should contain both changes, and preserve the trailing
        // newline exactly (the old line-merger stripped it).
        assert_eq!(merged_str, "Line 1\nOur Line 2\nLine 3\nTheir Line 4\n");
    }
    
    #[test]
    fn test_auto_merge_with_conflict() {
        let mut store = InMemoryObjectStore::new();

        // Base version
        let base_content = b"Line 1\nLine 2\nLine 3\nLine 4\n";
        let base_id = store.insert(base_content).unwrap();

        // Our version (modified line 2)
        let ours_content = b"Line 1\nOur Line 2\nLine 3\nLine 4\n";
        let ours_id = store.insert(ours_content).unwrap();

        // Their version (also modified line 2, but differently)
        let theirs_content = b"Line 1\nTheir Line 2\nLine 3\nLine 4\n";
        let theirs_id = store.insert(theirs_content).unwrap();

        // Auto-merge should fail due to conflict
        let result = auto_merge_files(&mut store, base_id, ours_id, theirs_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_auto_merge_conflicting_added_lines() {
        let mut store = InMemoryObjectStore::new();

        // Base version
        let base_content = b"Line 1\nLine 2\nLine 3\nLine 4\n";
        let base_id = store.insert(base_content).unwrap();

        // Both sides append a *different* line at the same position.
        let ours_content = b"Line 1\nLine 2\nLine 3\nLine 4\nOur Added Line\n";
        let ours_id = store.insert(ours_content).unwrap();
        let theirs_content = b"Line 1\nLine 2\nLine 3\nLine 4\nTheir Added Line\n";
        let theirs_id = store.insert(theirs_content).unwrap();

        // Two different insertions at the same point is a conflict, exactly as
        // git reports it. (The old bespoke merger silently concatenated both.)
        let result = auto_merge_files(&mut store, base_id, ours_id, theirs_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_auto_merge_independent_added_lines() {
        let mut store = InMemoryObjectStore::new();

        // Base version.
        let base_content = b"Line 1\nLine 2\nLine 3\nLine 4\n";
        let base_id = store.insert(base_content).unwrap();

        // Our side inserts near the top; their side appends at the bottom.
        // These do not overlap, so they merge cleanly.
        let ours_content = b"Line 1\nOur Added Line\nLine 2\nLine 3\nLine 4\n";
        let ours_id = store.insert(ours_content).unwrap();
        let theirs_content = b"Line 1\nLine 2\nLine 3\nLine 4\nTheir Added Line\n";
        let theirs_id = store.insert(theirs_content).unwrap();

        let result = auto_merge_files(&mut store, base_id, ours_id, theirs_id).unwrap();
        let merged_id = result.expect("independent inserts should auto-merge");
        let merged_content = store.read(merged_id).unwrap().unwrap();

        assert_eq!(
            merged_content,
            b"Line 1\nOur Added Line\nLine 2\nLine 3\nLine 4\nTheir Added Line\n"
        );
    }

    #[test]
    fn test_auto_merge_preserves_crlf_and_no_final_newline() {
        let mut store = InMemoryObjectStore::new();

        // CRLF line endings, and deliberately no newline after the last line.
        let base_id = store.insert(b"a\r\nb\r\nc").unwrap();
        let ours_id = store.insert(b"A\r\nb\r\nc").unwrap(); // change first line
        let theirs_id = store.insert(b"a\r\nb\r\nC").unwrap(); // change last line

        let merged_id = auto_merge_files(&mut store, base_id, ours_id, theirs_id)
            .unwrap()
            .expect("non-overlapping edits should merge");
        let merged = store.read(merged_id).unwrap().unwrap();

        // CRLF endings intact, still no trailing newline: byte-exact.
        assert_eq!(merged, b"A\r\nb\r\nC");
    }

    #[test]
    fn test_auto_merge_one_sided_change_takes_that_side() {
        let mut store = InMemoryObjectStore::new();
        let base_id = store.insert(b"x\ny\nz\n").unwrap();
        let ours_id = base_id; // we did not touch it
        let theirs_id = store.insert(b"x\nY\nz\n").unwrap();

        let merged_id = auto_merge_files(&mut store, base_id, ours_id, theirs_id)
            .unwrap()
            .unwrap();
        assert_eq!(merged_id, theirs_id);
    }
    
    #[test]
    fn test_merge_simple() {
        let mut store = InMemoryObjectStore::new();
        
        // Create a base snapshot
        let base_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![
                ("file1.txt", b"file1 content"),
                ("file2.txt", b"file2 content"),
                ("file3.txt", b"file3 content")
            ]
        );
        
        // Create our branch (modify file1, add file4)
        let ours_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"file1 modified by us"),
                ("file2.txt", b"file2 content"),
                ("file3.txt", b"file3 content"),
                ("file4.txt", b"file4 added by us")
            ]
        );
        
        // Create their branch (modify file2, add file5)
        let theirs_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"file1 content"),
                ("file2.txt", b"file2 modified by them"),
                ("file3.txt", b"file3 content"),
                ("file5.txt", b"file5 added by them")
            ]
        );
        
        // Perform the merge
        let result = merge(&mut store, base_id, ours_id, theirs_id, "ours", "theirs", false).unwrap();
        
        // Merge should succeed without conflicts
        assert!(result.success);
        assert!(result.conflicts.is_empty());
        assert!(result.merged_directory.is_some());
        
        // Check the merged directory
        let merged_dir = result.merged_directory.unwrap();
        
        // It should contain all files with their correct versions
        assert_eq!(merged_dir.root.len(), 5);
        
        // Check file1 (modified by us)
        if let Some(DirectoryEntry::File(id)) = merged_dir.root.get("file1.txt") {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"file1 modified by us");
        } else {
            panic!("file1.txt missing or not a file");
        }
        
        // Check file2 (modified by them)
        if let Some(DirectoryEntry::File(id)) = merged_dir.root.get("file2.txt") {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"file2 modified by them");
        } else {
            panic!("file2.txt missing or not a file");
        }
        
        // Check file3 (unchanged)
        if let Some(DirectoryEntry::File(id)) = merged_dir.root.get("file3.txt") {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"file3 content");
        } else {
            panic!("file3.txt missing or not a file");
        }
        
        // Check file4 (added by us)
        if let Some(DirectoryEntry::File(id)) = merged_dir.root.get("file4.txt") {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"file4 added by us");
        } else {
            panic!("file4.txt missing or not a file");
        }
        
        // Check file5 (added by them)
        if let Some(DirectoryEntry::File(id)) = merged_dir.root.get("file5.txt") {
            let content = store.read(*id).unwrap().unwrap();
            assert_eq!(content, b"file5 added by them");
        } else {
            panic!("file5.txt missing or not a file");
        }
    }

    #[test]
    fn test_merge_nested_directories() {
        let mut store = InMemoryObjectStore::new();

        // Helper function to check file content
        fn check_file_content<Store: ObjectStore>(
            store: &Store,
            dir: &Directory,
            path: &str,
            expected: &[u8]
        ) where Store::Error: std::fmt::Debug {
            let parts: Vec<&str> = path.split('/').collect();
            let mut current = dir;
            let mut current_path = Vec::new();
            
            // Navigate through directories to the file
            for (i, part) in parts.iter().enumerate() {
                if i == parts.len() - 1 {
                    // We're at the file
                    if let Some(DirectoryEntry::File(id)) = current.root.get(*part) {
                        let content = store.read(*id).unwrap().unwrap();
                        assert_eq!(content, expected, "Content mismatch for {}", path);
                    } else {
                        panic!("File not found: {}", path);
                    }
                } else {
                    // Navigate to subdirectory
                    current_path.push(*part);
                    if let Some(DirectoryEntry::Directory(subdir)) = current.root.get(*part) {
                        current = subdir;
                    } else {
                        panic!("Directory not found: {}", current_path.join("/"));
                    }
                }
            }
        }
        
        // Create a base snapshot with nested directories
        let base_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![
                ("file1.txt", b"root file"),
                ("dir1/file2.txt", b"dir1 file"),
                ("dir1/subdir/file3.txt", b"subdir file"),
                ("dir2/file4.txt", b"dir2 file")
            ]
        );
        
        // Create branch 1: modify files in multiple directories
        let branch1_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"root file modified in branch1"),
                ("dir1/file2.txt", b"dir1 file modified in branch1"),
                ("dir1/subdir/file3.txt", b"subdir file"),
                ("dir1/subdir/file5.txt", b"new file added in branch1"),
                ("dir2/file4.txt", b"dir2 file")
            ]
        );
        
        // Create branch 2: modify different files and add a new subdirectory
        let branch2_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"root file"),
                ("dir1/file2.txt", b"dir1 file"),
                ("dir1/subdir/file3.txt", b"subdir file modified in branch2"),
                ("dir2/file4.txt", b"dir2 file modified in branch2"),
                ("dir2/newdir/file6.txt", b"new nested file added in branch2")
            ]
        );
        
        // Perform the merge
        let result = merge(&mut store, base_id, branch1_id, branch2_id, "branch1", "branch2", false).unwrap();
        
        // Merge should succeed without conflicts
        assert!(result.success, "Merge failed with conflicts: {:?}", result.conflicts);
        assert!(result.conflicts.is_empty());
        assert!(result.merged_directory.is_some());
        
        // Check the merged directory structure
        let merged_dir = result.merged_directory.unwrap();
        
        // Verify all files are present with correct content
        check_file_content(&store, &merged_dir, "file1.txt", b"root file modified in branch1");
        check_file_content(&store, &merged_dir, "dir1/file2.txt", b"dir1 file modified in branch1");
        check_file_content(&store, &merged_dir, "dir1/subdir/file3.txt", b"subdir file modified in branch2");
        check_file_content(&store, &merged_dir, "dir1/subdir/file5.txt", b"new file added in branch1");
        check_file_content(&store, &merged_dir, "dir2/file4.txt", b"dir2 file modified in branch2");
        check_file_content(&store, &merged_dir, "dir2/newdir/file6.txt", b"new nested file added in branch2");
    }
    
    #[test]
    fn test_merge_with_strategy() {
        let mut store = InMemoryObjectStore::new();

        // Create a base snapshot with a file
        let base_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![
                ("file1.txt", b"Original content"),
                ("file2.txt", b"Unchanged content"),
            ]
        );

        // Create our branch (modify file1.txt)
        let ours_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"Our modified content"),
                ("file2.txt", b"Unchanged content"),
            ]
        );

        // Create their branch (modify file1.txt differently)
        let theirs_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"Their modified content"),
                ("file2.txt", b"Unchanged content"),
            ]
        );

        // Perform the merge
        let merge_result = merge(&mut store, base_id, ours_id, theirs_id, "main", "feature", false).unwrap();

        // Verify that it has a conflict
        assert!(!merge_result.success);
        assert_eq!(merge_result.conflicts.len(), 1);
        assert_eq!(merge_result.conflicts[0].conflict_type, ConflictType::BothModified);

        // Manually resolve the conflict with 'ours' strategy
        let resolver = ConflictResolver::new(&store, merge_result.conflicts[0].clone(), "main", "feature").unwrap();
        let ours_resolved_id = resolver.resolve_with_strategy(&mut store, MergeStrategy::Ours).unwrap().unwrap();

        // Create a copy of the merge result with the resolved conflict
        let mut resolved_conflicts = merge_result.conflicts.clone();
        resolved_conflicts[0].resolved = true;
        resolved_conflicts[0].resolution_id = Some(ours_resolved_id);

        let mut resolved_result = merge_result.clone();
        resolved_result.conflicts = resolved_conflicts;

        // Check if we can create a merged snapshot
        let snapshot_id = resolved_result.create_snapshot(&mut store, "Merge with ours strategy".to_string()).unwrap();

        // Verify the merged snapshot
        let snapshot: SnapShot = serde_json::from_slice(&store.read(snapshot_id).unwrap().unwrap()).unwrap();
        let directory: Directory = serde_json::from_slice(&store.read(snapshot.directory).unwrap().unwrap()).unwrap();

        // Check that the merged directory contains the right content
        if let Some(DirectoryEntry::File(file_id)) = directory.root.get("file1.txt") {
            let content = store.read(*file_id).unwrap().unwrap();
            assert_eq!(content, b"Our modified content"); // Used our version
        } else {
            panic!("file1.txt missing or not a file in merged result");
        }

        if let Some(DirectoryEntry::File(file_id)) = directory.root.get("file2.txt") {
            let content = store.read(*file_id).unwrap().unwrap();
            assert_eq!(content, b"Unchanged content"); // Unchanged file was preserved
        } else {
            panic!("file2.txt missing or not a file in merged result");
        }

        // Test with 'theirs' strategy on a new merge
        let merge_result2 = merge(&mut store, base_id, ours_id, theirs_id, "main", "feature", false).unwrap();

        // Manually resolve the conflict with 'theirs' strategy
        let resolver2 = ConflictResolver::new(&store, merge_result2.conflicts[0].clone(), "main", "feature").unwrap();
        let theirs_resolved_id = resolver2.resolve_with_strategy(&mut store, MergeStrategy::Theirs).unwrap().unwrap();

        // Create a copy of the merge result with the resolved conflict
        let mut resolved_conflicts2 = merge_result2.conflicts.clone();
        resolved_conflicts2[0].resolved = true;
        resolved_conflicts2[0].resolution_id = Some(theirs_resolved_id);

        let mut resolved_result2 = merge_result2.clone();
        resolved_result2.conflicts = resolved_conflicts2;

        // Check if we can create a merged snapshot
        let snapshot_id2 = resolved_result2.create_snapshot(&mut store, "Merge with theirs strategy".to_string()).unwrap();

        // Verify the merged snapshot
        let snapshot2: SnapShot = serde_json::from_slice(&store.read(snapshot_id2).unwrap().unwrap()).unwrap();
        let directory2: Directory = serde_json::from_slice(&store.read(snapshot2.directory).unwrap().unwrap()).unwrap();

        // Check that the merged directory contains the right content
        if let Some(DirectoryEntry::File(file_id)) = directory2.root.get("file1.txt") {
            let content = store.read(*file_id).unwrap().unwrap();
            assert_eq!(content, b"Their modified content"); // Used their version
        } else {
            panic!("file1.txt missing or not a file in merged result");
        }
    }

    #[test]
    fn test_merge_file_directory_conflict() {
        let mut store = InMemoryObjectStore::new();

        // Create a base snapshot
        let base_id = create_test_snapshot(
            &mut store,
            vec![],
            vec![
                ("file1.txt", b"file1 content"),
                ("file2.txt", b"file2 content")
            ]
        );

        // Create our branch: convert file2.txt to a directory
        let ours_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"file1 modified by us"),
                ("file2.txt/subfile1.txt", b"subfile in directory"),
                ("file2.txt/subfile2.txt", b"another subfile")
            ]
        );

        // Create their branch: modify file2.txt as a file
        let theirs_id = create_test_snapshot(
            &mut store,
            vec![base_id],
            vec![
                ("file1.txt", b"file1 content"),
                ("file2.txt", b"file2 modified by them")
            ]
        );

        // Perform the merge
        let result = merge(&mut store, base_id, ours_id, theirs_id, "ours", "theirs", false).unwrap();

        // Merge should fail due to type conflict
        assert!(!result.success);
        assert_eq!(result.conflicts.len(), 1);

        // Check conflict details
        let conflict = &result.conflicts[0];
        assert_eq!(conflict.path, PathBuf::from("file2.txt"));
        assert!(conflict.conflict_type == ConflictType::TypeChanged || 
               conflict.conflict_type == ConflictType::ModifiedDeleted,
               "Expected TypeChanged or ModifiedDeleted conflict, got {:?}", conflict.conflict_type);
    }
}