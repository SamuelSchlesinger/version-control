use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    content_diff::{Change, ContentDiff},
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

    /// Formats content with conflict markers for manual resolution,
    /// including base content for better context
    fn create_marked_content(
        base: &Option<Vec<u8>>,
        ours: &Option<Vec<u8>>,
        theirs: &Option<Vec<u8>>,
        our_branch: &str,
        their_branch: &str,
    ) -> Vec<u8> {
        let mut result = Vec::new();

        // Add a Git-style conflict marker header
        result.extend_from_slice(format!("<<<<<<< HEAD (Current branch: {})\n", our_branch).as_bytes());

        // Add our content if available
        if let Some(content) = ours {
            result.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                result.push(b'\n');
            }
        }

        // Add separator
        result.extend_from_slice(b"=======\n");

        // Add their content if available
        if let Some(content) = theirs {
            result.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                result.push(b'\n');
            }
        }

        // Add closer with branch name
        result.extend_from_slice(format!(">>>>>>> {} (Incoming changes)\n", their_branch).as_bytes());

        // Add base version for reference (similar to Git's conflict style with diff3)
        if let Some(content) = base {
            result.extend_from_slice(b"||||||| BASE (common ancestor)\n");
            result.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                result.push(b'\n');
            }
        }

        // Add a helpful comment at the end
        result.extend_from_slice(b"\n# CONFLICT RESOLUTION INSTRUCTIONS\n");
        result.extend_from_slice(b"# 1. Edit this file to resolve the conflict\n");
        result.extend_from_slice(b"# 2. Remove ALL conflict marker lines including:\n");
        result.extend_from_slice(b"#    - <<<<<<< HEAD\n");
        result.extend_from_slice(b"#    - =======\n");
        result.extend_from_slice(format!("#    - >>>>>>> {}\n", their_branch).as_bytes());
        result.extend_from_slice(b"#    - ||||||| BASE\n");
        result.extend_from_slice(b"#    - All lines starting with #\n");
        result.extend_from_slice(b"# 3. Save the file\n");
        result.extend_from_slice(b"# 4. Continue the merge with 'revtool merge --continue'\n");

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

    Ok(MergeResult {
        base_id,
        ours_id,
        theirs_id,
        merged_directory: if !has_conflicts { Some(merged_dir) } else { None },
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
    
    // Build maps of all ancestors for each snapshot with their "distance" from the original snapshot
    let ancestors1 = find_all_ancestors_with_distance(store, snapshot1_id)?;
    let ancestors2 = find_all_ancestors_with_distance(store, snapshot2_id)?;
    
    // Find common ancestors with their total distance from both snapshots
    let mut common_ancestors: Vec<(ObjectId, usize)> = Vec::new();
    for (ancestor, distance1) in &ancestors1 {
        if let Some(distance2) = ancestors2.get(ancestor) {
            common_ancestors.push((*ancestor, distance1 + distance2));
        }
    }
    
    // If no common ancestors found, return None
    if common_ancestors.is_empty() {
        return Ok(None);
    }
    
    // Sort by total distance (smaller distance = more recent common ancestor)
    common_ancestors.sort_by_key(|&(_, distance)| distance);
    
    // Return the most recent common ancestor (smallest total distance)
    Ok(Some(common_ancestors[0].0))
}

/// Find all ancestors of a snapshot with their distance (number of commits) from the original snapshot
fn find_all_ancestors_with_distance<Store: ObjectStore>(
    store: &Store,
    snapshot_id: ObjectId,
) -> Result<BTreeMap<ObjectId, usize>, Error<Store>> {
    let mut ancestors = BTreeMap::new();
    // Queue of (snapshot_id, distance)
    let mut queue = vec![(snapshot_id, 0)];
    
    while let Some((current_id, distance)) = queue.pop() {
        if ancestors.contains_key(&current_id) {
            continue;
        }
        
        ancestors.insert(current_id, distance);
        
        // Get parents of this snapshot
        let snapshot_bytes = match store.read(current_id).map_err(Error::Store)? {
            Some(bytes) => bytes,
            None => continue, // Skip if we can't read this snapshot
        };
        
        let snapshot: SnapShot = match serde_json::from_slice(&snapshot_bytes) {
            Ok(snap) => snap,
            Err(_) => continue, // Skip if we can't parse this snapshot
        };
        
        // Add parents to the queue with incremented distance
        for parent_id in snapshot.previous {
            if !ancestors.contains_key(&parent_id) {
                queue.push((parent_id, distance + 1));
            }
        }
    }
    
    Ok(ancestors)
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
                        // This shouldn't happen in theory, but handle it just in case
                        // Create an empty content hash
                        let empty_content = Vec::<u8>::new();
                        return Err(Error::ObjectMissing(ObjectId::from(&empty_content[..])));
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

/// Attempts to automatically merge two files
/// Returns the ID of the merged file if successful, or None if conflicts were detected
fn auto_merge_files<Store: ObjectStore>(
    store: &mut Store,
    base_id: ObjectId,
    ours_id: ObjectId,
    theirs_id: ObjectId,
) -> Result<Option<ObjectId>, Error<Store>> {
    // Check if files are identical
    if ours_id == theirs_id {
        return Ok(Some(ours_id));
    }

    // Generate content diffs
    let ours_diff_result = ContentDiff::generate(store, base_id, ours_id);
    let ours_diff = match ours_diff_result {
        Ok(Some(diff)) => diff,
        Ok(None) => return Ok(Some(ours_id)), // No changes from base to ours
        Err(e) => return Err(Error::ContentDiff(e)),
    };

    let theirs_diff_result = ContentDiff::generate(store, base_id, theirs_id);
    let theirs_diff = match theirs_diff_result {
        Ok(Some(diff)) => diff,
        Ok(None) => return Ok(Some(ours_id)), // No changes from base to theirs
        Err(e) => return Err(Error::ContentDiff(e)),
    };
    
    // Load file contents
    let base_content = store.read(base_id).map_err(Error::Store)?
        .ok_or(Error::ObjectMissing(base_id))?;
    let base_str = String::from_utf8_lossy(&base_content);
    let base_lines: Vec<&str> = base_str.lines().collect();
    
    // Create line-based change maps
    let ours_changes = create_line_change_map(&ours_diff);
    let theirs_changes = create_line_change_map(&theirs_diff);
    
    // Check for overlapping changes (conflicts)
    for (line_num, our_change) in &ours_changes {
        if let Some(their_change) = theirs_changes.get(line_num) {
            // Two added lines at the same position isn't a conflict
            if let (Change::Added(_), Change::Added(_)) = (our_change, their_change) {
                continue; // Not a conflict, handle during merge
            }
            
            if our_change != their_change {
                // Conflict: both changed the same line differently
                return Ok(None);
            }
        }
    }
    
    // No conflicts, proceed with auto-merge
    let mut merged_lines = Vec::new();
    
    // Create a combined set of line numbers to process in order
    let mut all_line_nums: BTreeSet<usize> = BTreeSet::new();
    for i in 0..base_lines.len() {
        all_line_nums.insert(i);
    }
    for line_num in ours_changes.keys().chain(theirs_changes.keys()) {
        all_line_nums.insert(*line_num);
    }
    
    // Process lines in order
    for i in all_line_nums {
        let base_line = if i < base_lines.len() {
            Some(base_lines[i].to_string())
        } else {
            None
        };
        
        // Check if we have changes for this line
        let our_change = ours_changes.get(&i);
        let their_change = theirs_changes.get(&i);
        
        match (our_change, their_change) {
            // Both branches added content at the same insertion point
            (Some(Change::Added(our_line)), Some(Change::Added(their_line))) => {
                // Add both lines in a reasonable order
                merged_lines.push(our_line.clone());
                merged_lines.push(their_line.clone());
            },
            
            // Our branch added a line
            (Some(Change::Added(line)), _) => {
                merged_lines.push(line.clone());
            },
            
            // Their branch added a line
            (_, Some(Change::Added(line))) => {
                merged_lines.push(line.clone());
            },
            
            // Our branch removed a line
            (Some(Change::Removed(_)), None) => {
                // Skip this line
            },
            
            // Their branch removed a line
            (None, Some(Change::Removed(_))) => {
                // Skip this line
            },
            
            // Our branch modified a line
            (Some(Change::Modified { new, .. }), None) => {
                merged_lines.push(new.clone());
            },
            
            // Their branch modified a line
            (None, Some(Change::Modified { new, .. })) => {
                merged_lines.push(new.clone());
            },
            
            // Both branches made the same modification
            (Some(Change::Modified { new, .. }), Some(Change::Modified { .. })) => {
                // We already checked earlier that these are equal
                merged_lines.push(new.clone());
            },
            
            // Both branches removed the same line
            (Some(Change::Removed(_)), Some(Change::Removed(_))) => {
                // Skip this line
            },
            
            // Context or unchanged line
            (None, None) => {
                if let Some(line) = base_line {
                    merged_lines.push(line);
                }
            },
            
            // Other combinations are either already checked for conflicts
            // or shouldn't occur in a valid diff
            _ => {},
        }
    }
    
    // Write the merged content
    let merged_content = merged_lines.join("\n").into_bytes();
    let merged_id = store.insert(&merged_content).map_err(Error::Store)?;
    
    Ok(Some(merged_id))
}

/// Creates a map of line number to change for efficient merging
fn create_line_change_map(diff: &ContentDiff) -> BTreeMap<usize, Change> {
    let mut changes = BTreeMap::new();
    let mut line_num = 0;
    let mut added_lines = Vec::new(); // Track added lines separately with their insertion points
    
    // First pass: track regular changes and collect added lines
    for change in &diff.changes {
        match change {
            Change::Added(line) => {
                // Store added lines with their insertion point for later processing
                added_lines.push((line_num, line.clone()));
                // Don't increment line_num for added lines
            },
            Change::Removed(line) => {
                changes.insert(line_num, Change::Removed(line.clone()));
                line_num += 1;
            },
            Change::Modified { old, new } => {
                changes.insert(line_num, Change::Modified { 
                    old: old.clone(), 
                    new: new.clone() 
                });
                line_num += 1;
            },
            Change::Context(_) => {
                line_num += 1;
            },
        }
    }
    
    // Second pass: process added lines with correct insertion points
    for (insertion_point, added_line) in added_lines {
        changes.insert(insertion_point, Change::Added(added_line));
    }
    
    changes
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
        let resolver = ConflictResolver::new(&store, conflict, "main", "feature").unwrap();

        // Check conflict markers in the marked content
        let marked_content = String::from_utf8_lossy(&resolver.marked_content);

        // Assert Git-style markers are present
        assert!(marked_content.contains("<<<<<<< HEAD (Current branch: main)"));
        assert!(marked_content.contains("======="));
        assert!(marked_content.contains(">>>>>>> feature (Incoming changes)"));
        assert!(marked_content.contains("||||||| BASE (common ancestor)"));
        assert!(marked_content.contains("# CONFLICT RESOLUTION INSTRUCTIONS"));

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
        
        // The merge should contain both changes
        assert_eq!(merged_str, "Line 1\nOur Line 2\nLine 3\nTheir Line 4");
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
    fn test_auto_merge_with_added_lines() {
        let mut store = InMemoryObjectStore::new();

        // Base version
        let base_content = b"Line 1\nLine 2\nLine 3\nLine 4\n";
        let base_id = store.insert(base_content).unwrap();

        // Our version (added a line at the end)
        let ours_content = b"Line 1\nLine 2\nLine 3\nLine 4\nOur Added Line\n";
        let ours_id = store.insert(ours_content).unwrap();

        // Their version (added a different line at the end)
        let theirs_content = b"Line 1\nLine 2\nLine 3\nLine 4\nTheir Added Line\n";
        let theirs_id = store.insert(theirs_content).unwrap();

        // Auto-merge should succeed
        let result = auto_merge_files(&mut store, base_id, ours_id, theirs_id).unwrap();
        assert!(result.is_some());

        // Check the merged content
        let merged_id = result.unwrap();
        let merged_content = store.read(merged_id).unwrap().unwrap();
        let merged_str = String::from_utf8_lossy(&merged_content);

        // The merge should include both added lines
        assert!(merged_str.contains("Our Added Line"));
        assert!(merged_str.contains("Their Added Line"));

        // The original lines should remain in order
        let merged_lines: Vec<&str> = merged_str.lines().collect();
        assert_eq!(merged_lines.len(), 6); // Base lines + 2 added lines
        assert_eq!(merged_lines[0], "Line 1");
        assert_eq!(merged_lines[1], "Line 2");
        assert_eq!(merged_lines[2], "Line 3");
        assert_eq!(merged_lines[3], "Line 4");
        
        // The next two lines should be our added lines in some order
        let has_our_line = merged_lines[4] == "Our Added Line" || merged_lines[5] == "Our Added Line";
        let has_their_line = merged_lines[4] == "Their Added Line" || merged_lines[5] == "Their Added Line";
        assert!(has_our_line && has_their_line);
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