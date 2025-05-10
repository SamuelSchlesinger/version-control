use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{object_id::ObjectId, object_store::ObjectStore};

/// Represents a single change in a file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// A line was added
    Added(String),
    /// A line was removed
    Removed(String),
    /// A line was changed from one value to another
    Modified {
        /// The original line
        old: String,
        /// The new line
        new: String,
    },
    /// Context line (unchanged)
    Context(String),
}

/// A set of changes to a file, represented as a sequence of line-by-line changes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDiff {
    /// The sequence of changes
    pub changes: Vec<Change>,
    /// The original object ID
    pub old_id: ObjectId,
    /// The new object ID
    pub new_id: ObjectId,
}

/// Error types for content diffing operations
#[derive(Debug)]
pub enum Error<Store: ObjectStore> {
    /// Object missing from store
    ObjectMissing(ObjectId),
    /// Store error
    Store(Store::Error),
    /// I/O error
    IO(std::io::Error),
}

impl<Store: ObjectStore> From<std::io::Error> for Error<Store> {
    fn from(error: std::io::Error) -> Self {
        Error::IO(error)
    }
}

impl ContentDiff {
    /// Generate a diff between two files identified by their object IDs
    pub fn generate<Store: ObjectStore>(
        store: &Store,
        old_id: ObjectId,
        new_id: ObjectId,
    ) -> Result<Option<Self>, Error<Store>> {
        // If IDs are the same, there's no diff
        if old_id == new_id {
            return Ok(None);
        }

        // Retrieve file contents from object store
        let old_content = match store.read(old_id).map_err(Error::Store)? {
            Some(content) => content,
            None => return Err(Error::ObjectMissing(old_id)),
        };

        let new_content = match store.read(new_id).map_err(Error::Store)? {
            Some(content) => content,
            None => return Err(Error::ObjectMissing(new_id)),
        };

        // Convert bytes to strings for diffing
        let old_str = String::from_utf8_lossy(&old_content);
        let new_str = String::from_utf8_lossy(&new_content);

        // Split into lines
        let old_lines: Vec<&str> = old_str.lines().collect();
        let new_lines: Vec<&str> = new_str.lines().collect();

        // Generate diff using the Myers diff algorithm
        let changes = diff_lines(&old_lines, &new_lines);

        Ok(Some(ContentDiff {
            changes,
            old_id,
            new_id,
        }))
    }

    /// Return the number of lines added in this diff
    pub fn added_lines(&self) -> usize {
        self.changes.iter().filter(|c| matches!(c, Change::Added(_))).count()
    }

    /// Return the number of lines removed in this diff
    pub fn removed_lines(&self) -> usize {
        self.changes.iter().filter(|c| matches!(c, Change::Removed(_))).count()
    }

    /// Return the number of lines modified in this diff
    pub fn modified_lines(&self) -> usize {
        self.changes.iter().filter(|c| matches!(c, Change::Modified { .. })).count()
    }
}

impl fmt::Display for ContentDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- {} (old)", self.old_id)?;
        writeln!(f, "+++ {} (new)", self.new_id)?;
        
        for change in &self.changes {
            match change {
                Change::Added(line) => writeln!(f, "+{}", line)?,
                Change::Removed(line) => writeln!(f, "-{}", line)?,
                Change::Modified { old, new } => {
                    writeln!(f, "-{}", old)?;
                    writeln!(f, "+{}", new)?;
                }
                Change::Context(line) => writeln!(f, " {}", line)?,
            }
        }
        
        Ok(())
    }
}

/// Compute the diff between two sequences of lines.
/// This implementation uses a simplified version of the Myers diff algorithm.
fn diff_lines(old_lines: &[&str], new_lines: &[&str]) -> Vec<Change> {
    // For a simplified implementation, we'll use a basic Longest Common Subsequence approach
    let lcs = longest_common_subsequence(old_lines, new_lines);
    let mut changes = Vec::new();
    
    let mut i = 0;
    let mut j = 0;
    
    for &idx in &lcs {
        // Process lines before this common element
        while i < idx.0 {
            changes.push(Change::Removed(old_lines[i].to_string()));
            i += 1;
        }
        
        while j < idx.1 {
            changes.push(Change::Added(new_lines[j].to_string()));
            j += 1;
        }
        
        // Add the common line as context
        changes.push(Change::Context(old_lines[i].to_string()));
        i += 1;
        j += 1;
    }
    
    // Process any remaining lines
    while i < old_lines.len() {
        changes.push(Change::Removed(old_lines[i].to_string()));
        i += 1;
    }
    
    while j < new_lines.len() {
        changes.push(Change::Added(new_lines[j].to_string()));
        j += 1;
    }
    
    // Merge adjacent removed/added lines into modified if they have a 1:1 correspondence
    let mut optimized_changes = Vec::new();
    let mut i = 0;
    
    while i < changes.len() {
        if i + 1 < changes.len() {
            if let (Change::Removed(old), Change::Added(new)) = (&changes[i], &changes[i + 1]) {
                optimized_changes.push(Change::Modified {
                    old: old.clone(),
                    new: new.clone(),
                });
                i += 2;
                continue;
            }
        }
        
        optimized_changes.push(changes[i].clone());
        i += 1;
    }
    
    // Add context lines if needed (typically 3 lines before and after changes)
    // This is a simplification - a real implementation would handle this more elegantly
    optimized_changes
}

/// Compute the Longest Common Subsequence between two sequences of lines.
/// Returns list of indices (old_idx, new_idx) representing matching lines.
fn longest_common_subsequence<'a>(old_lines: &[&'a str], new_lines: &[&'a str]) -> Vec<(usize, usize)> {
    let m = old_lines.len();
    let n = new_lines.len();
    
    // Create a 2D table to store LCS lengths
    let mut dp = vec![vec![0; n + 1]; m + 1];
    
    // Fill the dp table
    for i in 1..=m {
        for j in 1..=n {
            if old_lines[i - 1] == new_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = std::cmp::max(dp[i - 1][j], dp[i][j - 1]);
            }
        }
    }
    
    // Backtrack to find the actual subsequence indices
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    
    while i > 0 && j > 0 {
        if old_lines[i - 1] == new_lines[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    
    // Reverse the result to get the correct order
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_store::in_memory::InMemoryObjectStore;

    #[test]
    fn test_content_diff_same_file() {
        let mut store = InMemoryObjectStore::new();
        let content = b"Hello, world!";
        let id = store.insert(content).unwrap();
        
        let diff = ContentDiff::generate(&store, id, id).unwrap();
        assert!(diff.is_none());
    }
    
    #[test]
    fn test_content_diff_different_files() {
        let mut store = InMemoryObjectStore::new();
        let old_content = b"Hello, world!\nThis is a test.\nLine 3.";
        let new_content = b"Hello, world!\nThis is a modification.\nLine 3.\nNew line.";
        
        let old_id = store.insert(old_content).unwrap();
        let new_id = store.insert(new_content).unwrap();
        
        let diff = ContentDiff::generate(&store, old_id, new_id).unwrap().unwrap();
        
        assert_eq!(diff.old_id, old_id);
        assert_eq!(diff.new_id, new_id);
        assert_eq!(diff.added_lines(), 1);
        assert_eq!(diff.removed_lines(), 0);
        assert_eq!(diff.modified_lines(), 1);
        
        // Check the actual diff content
        assert!(matches!(diff.changes[0], Change::Context(_)));
        assert!(matches!(diff.changes[1], Change::Modified { .. }));
        assert!(matches!(diff.changes[2], Change::Context(_)));
        assert!(matches!(diff.changes[3], Change::Added(_)));
    }
    
    #[test]
    fn test_lcs() {
        let old = &["A", "B", "C", "X", "D", "E"];
        let new = &["A", "B", "Y", "C", "D", "Z", "E"];
        
        let lcs = longest_common_subsequence(old, new);
        
        // Expected LCS indices: A(0,0), B(1,1), C(2,3), D(4,4), E(5,6)
        assert_eq!(lcs.len(), 5);
        assert_eq!(lcs[0], (0, 0)); // A
        assert_eq!(lcs[1], (1, 1)); // B
        assert_eq!(lcs[2], (2, 3)); // C
        assert_eq!(lcs[3], (4, 4)); // D
        assert_eq!(lcs[4], (5, 6)); // E
    }
}