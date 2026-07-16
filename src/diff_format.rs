use std::fmt;

use crate::{
    content_diff::{Change, ContentDiff},
    directory::{Diff, DiffEntry, DirectoryEntry},
    snapshot_diff::SnapShotDiff,
};

/// Renders a path for display, quoting and escaping it when it contains control
/// characters or a double quote. A filename may legally contain a newline;
/// printed raw it forges what looks like a second status/diff line (a file named
/// `evil.txt\nD real.txt` renders as two entries). Quoting neutralizes that.
fn escape_path(s: &str) -> String {
    if s.chars().any(|c| c.is_control() || c == '"') {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        s.to_string()
    }
}

/// Flattens an added entry to the individual files it introduces, so an added
/// directory is reported file-by-file (`A dir/a.txt`, `A dir/b.txt`) rather than
/// collapsed to a single opaque `A dir` line. An empty directory yields itself.
fn flatten_added_leaves(prefix: &str, entry: &DirectoryEntry, out: &mut Vec<String>) {
    match entry {
        DirectoryEntry::File(_) => out.push(prefix.to_string()),
        DirectoryEntry::Directory(dir) if dir.root.is_empty() => out.push(prefix.to_string()),
        DirectoryEntry::Directory(dir) => {
            for (name, child) in &dir.root {
                flatten_added_leaves(&format!("{prefix}/{name}"), child, out);
            }
        }
    }
}

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

    /// Wraps `text` in an ANSI color when color is enabled, otherwise returns it
    /// unchanged. Collapses the pervasive `if use_color { .. } else { .. }`
    /// duplication that this formatter used to repeat at every output site.
    fn colorize(&self, color: &str, text: &str) -> String {
        if self.options.use_color {
            format!("{color}{text}{}", Colors::RESET)
        } else {
            text.to_string()
        }
    }

    /// Format the diff as a string
    pub fn format(&self) -> String {
        let mut result = String::new();

        // Show stats if enabled
        if self.options.show_stats {
            self.format_stats(&mut result);
        }

        // Get sorted paths for consistent output. Added directories are
        // expanded to the individual files they introduce so nothing is hidden
        // behind a single collapsed `A dir` line.
        let mut paths: Vec<(String, DiffAction)> = Vec::new();
        for path in &self.diff.deleted {
            paths.push((path.clone(), DiffAction::Deleted));
        }
        for (path, entry) in &self.diff.added {
            let mut leaves = Vec::new();
            flatten_added_leaves(path, entry, &mut leaves);
            for leaf in leaves {
                paths.push((leaf, DiffAction::Added));
            }
        }
        for path in self.diff.modified.keys() {
            paths.push((path.clone(), DiffAction::Modified));
        }
        paths.sort_by(|a, b| a.0.cmp(&b.0));

        // Format each path
        for (path, action) in &paths {
            let path = path.as_str();
            match action {
                DiffAction::Deleted => {
                    result.push_str(&format!("{}\n", self.colorize(Colors::RED, &format!("D {}", escape_path(path)))));
                }
                DiffAction::Added => {
                    result.push_str(&format!("{}\n", self.colorize(Colors::GREEN, &format!("A {}", escape_path(path)))));
                }
                DiffAction::Modified => {
                    if let Some(entry) = self.diff.modified.get(path) {
                        result.push_str(&format!("{}\n", self.colorize(Colors::YELLOW, &format!("M {}", escape_path(path)))));

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

                                        result.push_str(&format!("  │   {}\n",
                                            self.colorize(Colors::YELLOW, &format!("M {}", escape_path(file_name)))));

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

                                    // Process added files (expand added subdirectories to leaves)
                                    for (file_name, entry) in &nested_diff.added {
                                        let mut leaves = Vec::new();
                                        flatten_added_leaves(file_name, entry, &mut leaves);
                                        for leaf in leaves {
                                            result.push_str(&format!("  │   {}\n",
                                                self.colorize(Colors::GREEN, &format!("A {}", escape_path(&leaf)))));
                                        }
                                    }

                                    // Process deleted files
                                    for file_name in &nested_diff.deleted {
                                        result.push_str(&format!("  │   {}\n",
                                            self.colorize(Colors::RED, &format!("D {}", escape_path(file_name)))));
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
        // Count the actual files an added directory introduces, not the single
        // directory entry, so the summary doesn't undercount (adding a folder of
        // 5 files reported "1 added").
        let added: usize = self
            .diff
            .added
            .iter()
            .map(|(path, entry)| {
                let mut leaves = Vec::new();
                flatten_added_leaves(path, entry, &mut leaves);
                leaves.len()
            })
            .sum();
        let removed = self.diff.deleted.len();
        let modified = self.diff.modified.len();
        let total = added + removed + modified;
        let files = if total == 1 { "file" } else { "files" };

        let line = format!(
            "Summary: {total} {files} changed, {added} added, {removed} removed, {modified} modified"
        );
        result.push_str(&self.colorize(Colors::BOLD, &line));
        result.push_str("\n\n");
    }

    fn format_content_diff(&self, _path: &str, diff: &ContentDiff) -> String {
        let mut result = String::new();
        result.push_str("  │\n");

        // Assign real file line numbers over the *full* change list first, so
        // the displayed numbers reflect positions in the file rather than in
        // the (possibly context-filtered) output.
        let mut old_no = 1usize;
        let mut new_no = 1usize;
        let mut rows: Vec<NumberedRow> = Vec::with_capacity(diff.changes.len());
        for change in &diff.changes {
            match change {
                Change::Added(line) => {
                    rows.push(NumberedRow::Added { line: line.clone(), new_no });
                    new_no += 1;
                }
                Change::Removed(line) => {
                    rows.push(NumberedRow::Removed { line: line.clone(), old_no });
                    old_no += 1;
                }
                Change::Modified { old, new } => {
                    rows.push(NumberedRow::Modified {
                        old: old.clone(),
                        new: new.clone(),
                        old_no,
                        new_no,
                    });
                    old_no += 1;
                    new_no += 1;
                }
                Change::Context(line) => {
                    rows.push(NumberedRow::Context { line: line.clone(), old_no, new_no });
                    old_no += 1;
                    new_no += 1;
                }
            }
        }

        let display_rows = if self.options.context_lines > 0 {
            filter_context_rows(rows, self.options.context_lines)
        } else {
            rows
        };

        for row in &display_rows {
            match row {
                NumberedRow::Added { line, new_no } => {
                    if self.options.use_color {
                        result.push_str(&format!("  │ {:4} │      │ {}{}{}\n",
                            new_no, Colors::GREEN, line, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │ {:4} │      │ +{}\n", new_no, line));
                    }
                }
                NumberedRow::Removed { line, old_no } => {
                    if self.options.use_color {
                        result.push_str(&format!("  │      │ {:4} │ {}{}{}\n",
                            old_no, Colors::RED, line, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │      │ {:4} │ -{}\n", old_no, line));
                    }
                }
                NumberedRow::Modified { old, new, old_no, new_no } => {
                    if self.options.use_color {
                        result.push_str(&format!("  │      │ {:4} │ {}{}{}\n",
                            old_no, Colors::RED, old, Colors::RESET));
                        result.push_str(&format!("  │ {:4} │      │ {}{}{}\n",
                            new_no, Colors::GREEN, new, Colors::RESET));
                    } else {
                        result.push_str(&format!("  │      │ {:4} │ -{}\n", old_no, old));
                        result.push_str(&format!("  │ {:4} │      │ +{}\n", new_no, new));
                    }
                }
                NumberedRow::Context { line, old_no, new_no } => {
                    result.push_str(&format!("  │ {:4} │ {:4} │  {}\n", new_no, old_no, line));
                }
                NumberedRow::Separator => {
                    result.push_str("  │  ... │  ... │\n");
                }
            }
        }

        result.push_str("  │\n");
        result
    }
}

/// A diff row carrying the real file line numbers to display for it.
enum NumberedRow {
    Added { line: String, new_no: usize },
    Removed { line: String, old_no: usize },
    Modified { old: String, new: String, old_no: usize, new_no: usize },
    Context { line: String, old_no: usize, new_no: usize },
    /// A gap where unchanged lines were elided.
    Separator,
}

impl NumberedRow {
    fn is_change(&self) -> bool {
        !matches!(self, NumberedRow::Context { .. } | NumberedRow::Separator)
    }
}

/// Keep every changed row plus `context` rows around each change, collapsing
/// hidden gaps (including a leading one) into a single `...` separator.
fn filter_context_rows(rows: Vec<NumberedRow>, context: usize) -> Vec<NumberedRow> {
    if rows.len() <= 2 * context {
        return rows;
    }

    let mut include = vec![false; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        if row.is_change() {
            let start = i.saturating_sub(context);
            let end = std::cmp::min(i + context + 1, rows.len());
            for slot in include[start..end].iter_mut() {
                *slot = true;
            }
        }
    }

    let mut out = Vec::new();
    let mut gap_before = false;
    for (i, row) in rows.into_iter().enumerate() {
        if include[i] {
            if gap_before {
                out.push(NumberedRow::Separator);
                gap_before = false;
            }
            out.push(row);
        } else {
            gap_before = true;
        }
    }
    out
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
        
        assert!(output.contains("Summary: 3 files changed"));
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