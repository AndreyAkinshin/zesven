//! File selection using glob patterns.

use glob::Pattern;
use zesven::read::{Entry, EntrySelector};

/// Error type for file selector operations
#[derive(Debug)]
pub struct PatternError(pub String);

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid glob pattern: {}", self.0)
    }
}

impl std::error::Error for PatternError {}

/// File selector based on include and exclude glob patterns
pub struct FileSelector {
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
}

impl FileSelector {
    /// Creates a new file selector from pattern strings
    pub fn new(include: &[String], exclude: &[String]) -> Result<Self, PatternError> {
        let include = include
            .iter()
            .map(|p| Pattern::new(p).map_err(|e| PatternError(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;

        let exclude = exclude
            .iter()
            .map(|p| Pattern::new(p).map_err(|e| PatternError(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { include, exclude })
    }

    /// Checks if a path matches the selection criteria
    pub fn matches(&self, path: &str) -> bool {
        // If include patterns specified, at least one must match
        if !self.include.is_empty() && !self.include.iter().any(|p| p.matches(path)) {
            return false;
        }

        // None of the exclude patterns should match
        !self.exclude.iter().any(|p| p.matches(path))
    }

    /// Checks if a path matches the selection criteria (with options)
    pub fn matches_with_options(&self, path: &str) -> bool {
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        // If include patterns specified, at least one must match
        if !self.include.is_empty() && !self.include.iter().any(|p| p.matches_with(path, options)) {
            return false;
        }

        // None of the exclude patterns should match
        !self.exclude.iter().any(|p| p.matches_with(path, options))
    }
}

impl EntrySelector for FileSelector {
    fn select(&self, entry: &Entry) -> bool {
        self.matches(entry.path.as_str())
    }
}

impl EntrySelector for &FileSelector {
    fn select(&self, entry: &Entry) -> bool {
        self.matches(entry.path.as_str())
    }
}

/// Creates a file selector that matches everything
#[allow(dead_code)] // Part of public API
pub fn select_all() -> impl EntrySelector {
    zesven::read::SelectAll
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_include_patterns() {
        let selector =
            FileSelector::new(&["*.txt".to_string(), "docs/*".to_string()], &[]).unwrap();

        assert!(selector.matches("readme.txt"));
        assert!(selector.matches("docs/manual.pdf"));
        assert!(!selector.matches("image.png"));
    }

    #[test]
    fn test_exclude_patterns() {
        let selector = FileSelector::new(&[], &["*.log".to_string(), "tmp/*".to_string()]).unwrap();

        assert!(selector.matches("readme.txt"));
        assert!(!selector.matches("debug.log"));
        assert!(!selector.matches("tmp/cache.dat"));
    }

    #[test]
    fn test_include_and_exclude() {
        let selector =
            FileSelector::new(&["*.txt".to_string()], &["debug*.txt".to_string()]).unwrap();

        assert!(selector.matches("readme.txt"));
        assert!(!selector.matches("debug.txt"));
        assert!(!selector.matches("debug_extra.txt"));
        assert!(!selector.matches("image.png"));
    }

    #[test]
    fn test_no_patterns() {
        let selector = FileSelector::new(&[], &[]).unwrap();

        assert!(selector.matches("anything.txt"));
        assert!(selector.matches("any/path/file.ext"));
    }
}
