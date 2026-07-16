use std::fmt;

use crate::{
    content_diff::{Change, ContentDiff},
    directory::{Diff, DiffEntry},
    snapshot_diff::SnapShotDiff,
};

/// Color codes for terminal output
pub struct Colors;

impl Colors {
    pub const RED: &'static str = "\x1b[31m";
    pub const GREEN: &'static str = "\x1b[32m";
    pub const YELLOW: &'static str = "\x1b[33m";
    pub const BLUE: &'static str = "\x1b[34m";
    pub const RESET: &'static str = "\x1b[0m";
    pub const BOLD: &'static str = "\x1b[1m";
}

/// Format options for diff displays
#[derive(Debug, Clone, Copy)]
pub struct FormatOptions {
    /// Whether to use color in the output
    pub use_color: bool,
    /// Whether to show file content diffs
    pub show_content: bool,
    /// Number of context lines to show around changes
    pub context_lines: usize,
    /// Whether to show stats summary
    pub show_stats: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            use_color: true,
            show_content: true,
            context_lines: 3,
            show_stats: true,
        }
    }
}

/// A formatter for directory diffs
pub struct DiffFormatter<'a> {
    diff: &'a Diff,
    options: FormatOptions,
}

impl<'a> DiffFormatter<'a> {
    /// Create a new diff formatter
    pub fn new(diff: &'a Diff, options: FormatOptions) -> Self {
        Self { diff, options }
    }

    /// Format the diff as a string
    pub fn format(&self) -> String {
        let mut result = String::new();

        // Show stats if enabled
        if self.options.show_stats {
            self.format_stats(&mut result);
        }

        // Get sorted paths for consistent output
        let mut paths = Vec::new();
        for path in &self.diff.deleted {
            paths.push((path.as_str(), DiffAction::Deleted));
        }
        for path in self.diff.added.keys() {
            paths.push((path.as_str(), DiffAction::Added));
        }
        for path in self.diff.modified.keys() {
            paths.push((path.as_str(), DiffAction::Modified));
        }
        paths.sort_by(|a, b| a.0.cmp(b.0));

        // Format each path
        for (path, action) in paths {
            match action {
                DiffAction::Deleted => {
                    if self.options.use_color {
                        result.push_str(&format!("{}{} {}{}\n", 
                            Colors::RED, "D", path, Colors::RESET));
                    } else {
                        result.push_str(&format!("D {}\n", path));
                    }
                }
                DiffAction::Added => {
                    if let Some(_entry) = self.diff.added.get(path) {
                        if self.options.use_color {
                            result.push_str(&format!("{}{} {}{}\n", 
                                Colors::GREEN, "A", path, Colors::RESET));
                        } else {
                            result.push_str(&format!("A {}\n", path));
                        }
                    }
                }
                DiffAction::Modified => {
                    if let Some(entry) = self.diff.modified.get(path) {
                        if self.options.use_color {
                            result.push_str(&format!("{}{} {}{}\n", 
                                Colors::YELLOW, "M", path, Colors::RESET));
                        } else {
                            result.push_str(&format!("M {}\n", path));
                        }

                        // Show content diff if enabled
                        if self.options.show_content {
                            match entry {
                                DiffEntry::FileWithContentDiff { content_diff: Some(content_diff), .. } => {
                                    result.push_str(&self.format_content_diff(path, content_diff));
                                }
                                DiffEntry::Directory(nested_diff) => {
                                    // Recursively format nested directory diffs with the same options

                                    // We need a custom nested path display for files in subdirectories
                                    result.push_str("  │ Directory contents:\n");

                                    // Process nested files to show their content diffs
                                    // First, process all modified files with content diffs
                                    for (file_name, entry) in &nested_diff.modified {
                                        let nested_path = format!("{}/{}", path, file_name);

                                        if self.options.use_color {
                                            result.push_str(&format!("  │   {}{} {}{}\n",
                                                Colors::YELLOW, "M", file_name, Colors::RESET));
                                        } else {
                                            result.push_str(&format!("  │   M {}\n", file_name));
                                        }

                                        // Show content diff for this file if it has one
                                        match entry {
                                            DiffEntry::FileWithContentDiff { content_diff: Some(content_diff), .. } => {
                                                // Format the content diff with extra indentation
                                                let diff_text = self.format_content_diff(&nested_path, content_diff);
                                                for line in diff_text.lines() {
                                                    result.push_str(&format!("  │   {}\n", line));
                                                }
                                            },
                                            DiffEntry::Directory(sub_nested_diff) => {
                                                // Handle deeper nesting by recursive call with indentation
                                                let nested_formatter = DiffFormatter::new(sub_nested_diff, self.options);
                                                let sub_result = nested_formatter.format();
                                                if !sub_result.is_empty() {
                                                    result.push_str("  │     Directory contents:\n");
                                                    for line in sub_result.lines() {
                                                        if !line.is_empty() {
                                                            result.push_str(&format!("  │       {}\n", line));
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Process added files
                                    for file_name in nested_diff.added.keys() {
                                        if self.options.use_color {
                                            result.push_str(&format!("  │   {}{} {}{}\n",
                                                Colors::GREEN, "A", file_name, Colors::RESET));
                                        } else {
                                            result.push_str(&format!("  │   A {}\n", file_name));
                                        }
                                    }

                                    // Process deleted files
                                    for file_name in &nested_diff.deleted {
                                        if self.options.use_color {
                                            result.push_str(&format!("  │   {}{} {}{}\n",
                                                Colors::RED, "D", file_name, Colors::RESET));
                                        } else {
                                            result.push_str(&format!("  │   D {}\n", file_name));
                                        }
                                    }

                                    result.push_str("  │\n");
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        result
    }

    fn format_stats(&self, result: &mut String) {
        let total = self.diff.added.len() + self.diff.deleted.len() + self.diff.modified.len();
        
        if self.options.use_color {
            result.push_str(&format!("{}Summary: {} file(s) changed, {} added, {} removed, {} modified{}\n\n",
                Colors::BOLD,
                total,
                self.diff.added.len(),
                self.diff.deleted.len(),
                self.diff.modified.len(),
                Colors::RESET
            ));
        } else {
            result.push_str(&format!("Summary: {} file(s) changed, {} added, {} removed, {} modified\n\n",
                total,
                self.diff.added.len(),
                self.diff.deleted.len(),
                self.diff.modified.len()
            ));
        }
    }

    fn format_content_diff(&self, _path: &str, diff: &ContentDiff) -> String {
        let mut result = String::new();
        result.push_str("  │\n");

        // Filter changes to only show the specified number of context lines
        let filtered_changes = if self.options.context_lines > 0 {
            self.filter_context_lines(diff.changes.as_slice(), self.options.context_lines)
        } else {
            diff.changes.clone()
        };

        // Add a line number counter
        let mut line_num_old = 1;
        let mut line_num_new = 1;

        for change in filtered_changes {
            match &change {
                Change::Added(line) => {
                    if self.options.use_color {
                        result.push_str(&format!("  │ {:4} │      │ {}{}{}\n", 
                            line_num_new, Colors::GREEN, line, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │ {:4} │      │ +{}\n", line_num_new, line));
                    }
                    line_num_new += 1;
                }
                Change::Removed(line) => {
                    if self.options.use_color {
                        result.push_str(&format!("  │      │ {:4} │ {}{}{}\n", 
                            line_num_old, Colors::RED, line, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │      │ {:4} │ -{}\n", line_num_old, line));
                    }
                    line_num_old += 1;
                }
                Change::Modified { old, new } => {
                    if self.options.use_color {
                        result.push_str(&format!("  │      │ {:4} │ {}{}{}\n", 
                            line_num_old, Colors::RED, old, Colors::RESET));
                        result.push_str(&format!("  │ {:4} │      │ {}{}{}\n", 
                            line_num_new, Colors::GREEN, new, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │      │ {:4} │ -{}\n", line_num_old, old));
                        result.push_str(&format!("  │ {:4} │      │ +{}\n", line_num_new, new));
                    }
                    line_num_old += 1;
                    line_num_new += 1;
                }
                Change::Context(line) => {
                    result.push_str(&format!("  │ {:4} │ {:4} │  {}\n", 
                        line_num_new, line_num_old, line));
                    line_num_old += 1;
                    line_num_new += 1;
                }
            }
        }
        
        result.push_str("  │\n");
        result
    }

    /// Filter context lines to only show relevant parts of the diff
    fn filter_context_lines(&self, changes: &[Change], context_lines: usize) -> Vec<Change> {
        // If no context filtering is needed, return the original changes
        if context_lines == 0 || changes.len() <= 2 * context_lines {
            return changes.to_vec();
        }

        let mut result = Vec::new();

        // Find all change positions (non-context lines)
        let mut change_positions = Vec::new();
        for (i, change) in changes.iter().enumerate() {
            match change {
                Change::Added(_) | Change::Removed(_) | Change::Modified { .. } => {
                    change_positions.push(i);
                },
                _ => {}
            }
        }

        // Track which lines we should include
        let mut include_lines = vec![false; changes.len()];

        // For each change, include context lines around it
        for &pos in &change_positions {
            // Include context before
            let start = pos.saturating_sub(context_lines);
            for slot in include_lines[start..pos].iter_mut() {
                *slot = true;
            }

            // Include the change itself
            include_lines[pos] = true;

            // Include context after
            let end = std::cmp::min(pos + context_lines + 1, changes.len());
            for slot in include_lines[pos + 1..end].iter_mut() {
                *slot = true;
            }
        }

        // Add separator markers where needed
        let mut separator_needed = false;
        for i in 0..changes.len() {
            if include_lines[i] {
                if separator_needed {
                    result.push(Change::Context("...".to_string()));
                    separator_needed = false;
                }
                result.push(changes[i].clone());
            } else if i > 0 && include_lines[i-1] {
                separator_needed = true;
            }
        }

        result
    }
}

impl<'a> fmt::Display for DiffFormatter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format())
    }
}

/// A formatter for snapshot diffs
pub struct SnapShotDiffFormatter<'a> {
    diff: &'a SnapShotDiff,
    options: FormatOptions,
}

impl<'a> SnapShotDiffFormatter<'a> {
    /// Create a new snapshot diff formatter
    pub fn new(diff: &'a SnapShotDiff, options: FormatOptions) -> Self {
        Self { diff, options }
    }

    /// Format the snapshot diff as a string
    pub fn format(&self) -> String {
        let mut result = String::new();

        // Header
        if self.options.use_color {
            result.push_str(&format!("{}Snapshot Diff{}\n", Colors::BOLD, Colors::RESET));
            result.push_str(&format!("{}Source:{} {} - {}\n", 
                Colors::BOLD, Colors::RESET, self.diff.source_id, self.diff.source_message));
            result.push_str(&format!("{}Target:{} {} - {}\n\n", 
                Colors::BOLD, Colors::RESET, self.diff.target_id, self.diff.target_message));
        } else {
            result.push_str("Snapshot Diff\n");
            result.push_str(&format!("Source: {} - {}\n", self.diff.source_id, self.diff.source_message));
            result.push_str(&format!("Target: {} - {}\n\n", self.diff.target_id, self.diff.target_message));
        }

        // Format the directory diff
        let dir_formatter = DiffFormatter::new(&self.diff.directory_diff, self.options);
        result.push_str(&dir_formatter.format());

        result
    }
}

impl<'a> fmt::Display for SnapShotDiffFormatter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format())
    }
}

/// Represents the type of diff action on a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffAction {
    Added,
    Deleted,
    Modified,
}

/// Shorthand function to format a diff with default options
pub fn format_diff(diff: &Diff) -> String {
    DiffFormatter::new(diff, FormatOptions::default()).format()
}

/// Shorthand function to format a snapshot diff with default options
pub fn format_snapshot_diff(diff: &SnapShotDiff) -> String {
    SnapShotDiffFormatter::new(diff, FormatOptions::default()).format()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        directory::{Diff, DirectoryEntry},
        content_diff::{Change, ContentDiff},
        object_id::ObjectId,
    };
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn test_diff_formatter() {
        let mut added = BTreeMap::new();
        added.insert("file1.txt".to_string(), DirectoryEntry::File(ObjectId::from(&vec![1, 2, 3])));
        
        let mut deleted = BTreeSet::new();
        deleted.insert("file2.txt".to_string());
        
        let mut modified = BTreeMap::new();
        modified.insert("file3.txt".to_string(), DiffEntry::File(ObjectId::from(&vec![4, 5, 6])));
        
        let diff = Diff {
            added,
            deleted,
            modified,
        };
        
        let options = FormatOptions {
            use_color: false,
            show_content: false,
            context_lines: 3,
            show_stats: true,
        };
        
        let formatter = DiffFormatter::new(&diff, options);
        let output = formatter.format();
        
        assert!(output.contains("Summary: 3 file(s) changed"));
        assert!(output.contains("A file1.txt"));
        assert!(output.contains("D file2.txt"));
        assert!(output.contains("M file3.txt"));
    }
    
    #[test]
    fn test_content_diff_formatter() {
        let added = BTreeMap::new();
        let mut modified = BTreeMap::new();
        
        // Create a content diff
        let changes = vec![
            Change::Context("Line 1".to_string()),
            Change::Context("Line 2".to_string()),
            Change::Removed("Line 3 - old".to_string()),
            Change::Added("Line 3 - new".to_string()),
            Change::Context("Line 4".to_string()),
        ];
        
        let content_diff = ContentDiff {
            changes,
            old_id: ObjectId::from(&vec![1, 2, 3]),
            new_id: ObjectId::from(&vec![4, 5, 6]),
        };
        
        modified.insert("file.txt".to_string(), DiffEntry::FileWithContentDiff {
            new_id: ObjectId::from(&vec![4, 5, 6]),
            content_diff: Some(content_diff),
        });
        
        let diff = Diff {
            added,
            deleted: BTreeSet::new(),
            modified,
        };
        
        let options = FormatOptions {
            use_color: false,
            show_content: true,
            context_lines: 3,
            show_stats: true,
        };
        
        let formatter = DiffFormatter::new(&diff, options);
        let output = formatter.format();
        
        assert!(output.contains("M file.txt"));
        assert!(output.contains("Line 1"));
        assert!(output.contains("Line 2"));
        assert!(output.contains("-Line 3 - old"));
        assert!(output.contains("+Line 3 - new"));
        assert!(output.contains("Line 4"));
    }
    
    #[test]
    fn test_context_filtering() {
        let mut modified = BTreeMap::new();
        
        // Create a content diff with many lines
        let mut changes = Vec::new();
        
        // Add 20 context lines
        for i in 1..=20 {
            changes.push(Change::Context(format!("Context line {}", i)));
        }
        
        // Add a change
        changes.push(Change::Removed("Removed line".to_string()));
        changes.push(Change::Added("Added line".to_string()));
        
        // Add 20 more context lines
        for i in 21..=40 {
            changes.push(Change::Context(format!("Context line {}", i)));
        }
        
        // Add another change
        changes.push(Change::Removed("Another removed line".to_string()));
        changes.push(Change::Added("Another added line".to_string()));
        
        // Add 10 more context lines
        for i in 41..=50 {
            changes.push(Change::Context(format!("Context line {}", i)));
        }
        
        let content_diff = ContentDiff {
            changes,
            old_id: ObjectId::from(&vec![1, 2, 3]),
            new_id: ObjectId::from(&vec![4, 5, 6]),
        };
        
        modified.insert("file.txt".to_string(), DiffEntry::FileWithContentDiff {
            new_id: ObjectId::from(&vec![4, 5, 6]),
            content_diff: Some(content_diff),
        });
        
        let diff = Diff {
            added: BTreeMap::new(),
            deleted: BTreeSet::new(),
            modified,
        };
        
        // Test with 3 context lines
        let options = FormatOptions {
            use_color: false,
            show_content: true,
            context_lines: 3,
            show_stats: true,
        };
        
        let formatter = DiffFormatter::new(&diff, options);
        let output = formatter.format();
        
        // Uncomment for debugging if needed
        // println!("TEST OUTPUT:\n{}", output);

        // Test that we have the change sections and some context around them
        assert!(output.contains("Context line 18"));
        assert!(output.contains("Context line 20"));
        assert!(output.contains("Removed line"));
        assert!(output.contains("Added line"));
        assert!(output.contains("Context line 23"));
        assert!(output.contains("..."));
        assert!(output.contains("Context line 38"));
        assert!(output.contains("Context line 40"));
        assert!(output.contains("Another removed line"));
        assert!(output.contains("Another added line"));
        assert!(output.contains("Context line 43"));
        
        // We're just going to check the presence of the important content
        // and not worry about the absence of other content that could show up in line numbers
        assert!(output.contains("Context line 18"));  // Verify we got this part
    }
}