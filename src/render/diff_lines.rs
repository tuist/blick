use std::collections::{BTreeMap, BTreeSet};

/// Map of file path → set of line numbers (in the *new* version of the file)
/// that appear in the diff hunks. Findings whose `(file, line)` is in this
/// set can be safely posted as inline review comments; ones outside the set
/// must go in the review body instead.
#[derive(Debug, Default, Clone)]
pub struct DiffLineIndex {
    files: BTreeMap<String, BTreeSet<u64>>,
}

impl DiffLineIndex {
    pub fn from_unified(diff: &str) -> Self {
        let mut index = DiffLineIndex::default();
        let mut current_file: Option<String> = None;
        let mut new_line: Option<u64> = None;

        for line in diff.split_inclusive('\n') {
            let line = line.trim_end_matches(['\n', '\r']);

            if let Some(rest) = line.strip_prefix("+++ b/") {
                current_file = Some(rest.to_owned());
                continue;
            }
            if line.starts_with("+++ /dev/null") {
                current_file = None;
                continue;
            }
            if line.starts_with("--- ")
                || line.starts_with("diff --git ")
                || line.starts_with("index ")
            {
                continue;
            }

            if let Some(rest) = line.strip_prefix("@@") {
                // Parse `+new_start[,new_count]` from the hunk header.
                if let Some((header, _)) = rest.split_once("@@") {
                    new_line = parse_hunk_new_start(header);
                }
                continue;
            }

            let Some(file) = current_file.as_deref() else {
                continue;
            };

            if line.starts_with('+') && !line.starts_with("+++") {
                if let Some(ln) = new_line {
                    index.files.entry(file.to_owned()).or_default().insert(ln);
                    new_line = Some(ln + 1);
                }
            } else if line.starts_with('-') && !line.starts_with("---") {
                // Removed line — does not consume a new-side line number.
            } else if let Some(ln) = new_line {
                // Context line: advances new line counter, also part of the
                // diff "view" so review comments are valid here too.
                index.files.entry(file.to_owned()).or_default().insert(ln);
                new_line = Some(ln + 1);
            }
        }

        index
    }

    pub fn contains(&self, file: &str, line: u64) -> bool {
        self.files
            .get(file)
            .map(|set| set.contains(&line))
            .unwrap_or(false)
    }

    pub fn knows_file(&self, file: &str) -> bool {
        self.files.contains_key(file)
    }
}

fn parse_hunk_new_start(header: &str) -> Option<u64> {
    // header looks like " -1,3 +5,7 " (with possible spaces/dashes around).
    for token in header.split_whitespace() {
        if let Some(rest) = token.strip_prefix('+') {
            let value = rest.split_once(',').map(|(s, _)| s).unwrap_or(rest);
            return value.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = "diff --git a/src/main.rs b/src/main.rs\n\
index 7527576..4d2bf4b 100644\n\
--- a/src/main.rs\n\
+++ b/src/main.rs\n\
@@ -1,3 +1,3 @@\n\
 fn main() {\n\
-    println!(\"hello\");\n\
+    println!(\"hello, blick\");\n\
 }\n";

    #[test]
    fn indexes_added_and_context_lines() {
        let index = DiffLineIndex::from_unified(SAMPLE_DIFF);
        assert!(index.contains("src/main.rs", 1));
        assert!(index.contains("src/main.rs", 2));
        assert!(index.contains("src/main.rs", 3));
        assert!(!index.contains("src/main.rs", 4));
        assert!(index.knows_file("src/main.rs"));
    }

    #[test]
    fn ignores_removed_only_files() {
        let diff = "diff --git a/old.txt b/old.txt\n\
deleted file mode 100644\n\
--- a/old.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-stale content\n";
        let index = DiffLineIndex::from_unified(diff);
        assert!(!index.knows_file("old.txt"));
    }

    #[test]
    fn parses_multiple_hunks_in_a_file() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n\
--- a/src/lib.rs\n\
+++ b/src/lib.rs\n\
@@ -1,3 +1,4 @@\n\
 a\n\
+b\n\
 c\n\
 d\n\
@@ -10,2 +11,3 @@\n\
 e\n\
+f\n\
 g\n";
        let index = DiffLineIndex::from_unified(diff);
        assert!(index.contains("src/lib.rs", 1));
        assert!(index.contains("src/lib.rs", 2));
        assert!(index.contains("src/lib.rs", 11));
        assert!(index.contains("src/lib.rs", 12));
    }
}
