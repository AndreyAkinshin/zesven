//! Extraction and test options for archive operations.

use crate::format::streams::ResourceLimits;
use crate::progress::ProgressReporter;

#[cfg(feature = "aes")]
use crate::Password;

// Re-export PathSafety from the safety module where it's now defined
pub use crate::safety::PathSafety;

/// Policy for handling existing files during extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverwritePolicy {
    /// Return an error if the file exists.
    #[default]
    Error,
    /// Skip files that already exist.
    Skip,
    /// Overwrite existing files.
    Overwrite,
}

/// Policy for handling symbolic links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkPolicy {
    /// Forbid symbolic links (safest).
    #[default]
    Forbid,
    /// Allow symbolic links but validate their targets.
    ValidateTargets,
    /// Allow all symbolic links (use with caution).
    Allow,
}

/// Policy for filtering entries based on selector matches.
///
/// This enum determines whether entries matching a selector should be
/// included in or excluded from the operation.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::read::{FilterPolicy, SelectByRegex, ExtractOptions};
///
/// // Include only .txt files
/// let options = ExtractOptions::default()
///     .with_filter(SelectByRegex::new(r"\.txt$").unwrap(), FilterPolicy::Include);
///
/// // Exclude .log files
/// let options = ExtractOptions::default()
///     .with_filter(SelectByRegex::new(r"\.log$").unwrap(), FilterPolicy::Exclude);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterPolicy {
    /// Include entries that match the selector.
    ///
    /// Only entries that match the selector will be processed.
    #[default]
    Include,
    /// Exclude entries that match the selector.
    ///
    /// Entries that match the selector will be skipped.
    Exclude,
}

impl FilterPolicy {
    /// Returns true if this is an include policy.
    pub fn is_include(&self) -> bool {
        matches!(self, Self::Include)
    }

    /// Returns true if this is an exclude policy.
    pub fn is_exclude(&self) -> bool {
        matches!(self, Self::Exclude)
    }

    /// Applies the policy to a selector match result.
    ///
    /// For `Include` policy, returns `matched` as-is.
    /// For `Exclude` policy, returns the inverse of `matched`.
    pub fn apply(&self, matched: bool) -> bool {
        match self {
            Self::Include => matched,
            Self::Exclude => !matched,
        }
    }
}

/// Thread configuration for parallel operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Threads {
    /// Automatically determine thread count.
    #[default]
    Auto,
    /// Use a specific number of threads.
    ///
    /// The count must be non-zero. Use [`NonZeroUsize`](std::num::NonZeroUsize) to ensure
    /// this at compile time. If you have a value that might be zero, use
    /// [`Threads::count_or_single`] instead.
    Count(std::num::NonZeroUsize),
    /// Single-threaded operation.
    Single,
}

impl Threads {
    /// Creates a `Threads::Count` variant from a `usize`.
    ///
    /// Returns `Threads::Single` if the count is zero, otherwise returns
    /// `Threads::Count` with the specified thread count.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::read::Threads;
    ///
    /// // Zero becomes Single
    /// assert_eq!(Threads::count_or_single(0), Threads::Single);
    ///
    /// // Non-zero becomes Count
    /// assert_eq!(Threads::count_or_single(4).count(), 4);
    /// ```
    pub fn count_or_single(n: usize) -> Self {
        match std::num::NonZeroUsize::new(n) {
            Some(count) => Self::Count(count),
            None => Self::Single,
        }
    }

    /// Returns the actual thread count.
    ///
    /// # Thread Count Resolution
    ///
    /// - `Threads::Auto`: Returns the number of available CPUs, minimum 1
    /// - `Threads::Count(n)`: Returns `n.get()` (always >= 1 since NonZeroUsize)
    /// - `Threads::Single`: Returns 1
    pub fn count(&self) -> usize {
        match self {
            Self::Auto => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            Self::Count(n) => n.get(),
            Self::Single => 1,
        }
    }
}

/// Options for preserving file metadata during extraction.
#[derive(Debug, Clone, Default)]
pub struct PreserveMetadata {
    /// Preserve file modification times.
    pub modification_time: bool,
    /// Preserve file creation times (platform-dependent).
    pub creation_time: bool,
    /// Preserve file attributes (read-only, hidden, etc.).
    pub attributes: bool,
}

impl PreserveMetadata {
    /// Preserve all available metadata.
    pub fn all() -> Self {
        Self {
            modification_time: true,
            creation_time: true,
            attributes: true,
        }
    }

    /// Preserve no metadata.
    pub fn none() -> Self {
        Self::default()
    }

    /// Preserve both modification and creation times.
    ///
    /// This is the most common choice for preserving file timestamps
    /// without preserving attributes.
    pub fn times() -> Self {
        Self {
            modification_time: true,
            creation_time: true,
            attributes: false,
        }
    }

    /// Preserve only the modification time.
    ///
    /// Use this when you want to preserve the most critical timestamp
    /// (when the file was last modified) but not creation time or attributes.
    pub fn modification_time_only() -> Self {
        Self {
            modification_time: true,
            creation_time: false,
            attributes: false,
        }
    }
}

/// Options for extraction operations.
#[derive(Default)]
pub struct ExtractOptions {
    /// Policy for handling existing files.
    pub overwrite: OverwritePolicy,
    /// Path safety validation policy.
    pub path_safety: PathSafety,
    /// Symbolic link handling policy.
    pub link_policy: LinkPolicy,
    /// Resource limits for extraction.
    pub limits: ResourceLimits,
    /// Thread configuration.
    pub threads: Threads,
    /// Metadata preservation options.
    pub preserve_metadata: PreserveMetadata,
    /// Password for encrypted archives.
    #[cfg(feature = "aes")]
    pub password: Option<Password>,
    /// Progress reporter for tracking extraction progress (optional).
    pub progress: Option<Box<dyn ProgressReporter>>,
}

impl std::fmt::Debug for ExtractOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractOptions")
            .field("overwrite", &self.overwrite)
            .field("path_safety", &self.path_safety)
            .field("link_policy", &self.link_policy)
            .field("threads", &self.threads)
            .field("preserve_metadata", &self.preserve_metadata)
            .finish_non_exhaustive()
    }
}

impl ExtractOptions {
    /// Creates extraction options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the overwrite policy.
    pub fn overwrite(mut self, policy: OverwritePolicy) -> Self {
        self.overwrite = policy;
        self
    }

    /// Sets the path safety policy.
    pub fn path_safety(mut self, policy: PathSafety) -> Self {
        self.path_safety = policy;
        self
    }

    /// Sets the link policy.
    pub fn link_policy(mut self, policy: LinkPolicy) -> Self {
        self.link_policy = policy;
        self
    }

    /// Sets the resource limits.
    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets the thread configuration.
    pub fn threads(mut self, threads: Threads) -> Self {
        self.threads = threads;
        self
    }

    /// Sets the metadata preservation options.
    pub fn preserve_metadata(mut self, preserve: PreserveMetadata) -> Self {
        self.preserve_metadata = preserve;
        self
    }

    /// Sets the password for encrypted archives.
    #[cfg(feature = "aes")]
    pub fn password(mut self, password: impl Into<Password>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the progress reporter.
    pub fn progress(mut self, reporter: impl ProgressReporter + 'static) -> Self {
        self.progress = Some(Box::new(reporter));
        self
    }

    /// Clones all settings except the progress reporter.
    ///
    /// This is useful when you need to extract multiple archives with the same
    /// settings but can't clone the options due to the non-Clone progress reporter.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::read::{ExtractOptions, OverwritePolicy};
    ///
    /// let base = ExtractOptions::new().overwrite(OverwritePolicy::Skip);
    /// archive1.extract(dest1, (), &base.clone_settings())?;
    /// archive2.extract(dest2, (), &base.clone_settings())?;
    /// ```
    pub fn clone_settings(&self) -> Self {
        Self {
            overwrite: self.overwrite,
            path_safety: self.path_safety,
            link_policy: self.link_policy,
            limits: self.limits.clone(),
            threads: self.threads,
            preserve_metadata: self.preserve_metadata.clone(),
            #[cfg(feature = "aes")]
            password: self.password.clone(),
            progress: None, // Cannot clone Box<dyn ProgressReporter>
        }
    }
}

/// Options for test (integrity verification) operations.
#[derive(Default)]
pub struct TestOptions {
    /// Resource limits for testing.
    pub limits: ResourceLimits,
    /// Thread configuration.
    pub threads: Threads,
    /// Password for encrypted archives.
    #[cfg(feature = "aes")]
    pub password: Option<Password>,
    /// Progress reporter for tracking test progress (optional).
    pub progress: Option<Box<dyn ProgressReporter>>,
}

impl std::fmt::Debug for TestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestOptions")
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

impl TestOptions {
    /// Creates test options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the resource limits.
    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets the thread configuration.
    pub fn threads(mut self, threads: Threads) -> Self {
        self.threads = threads;
        self
    }

    /// Sets the password for encrypted archives.
    #[cfg(feature = "aes")]
    pub fn password(mut self, password: impl Into<Password>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the progress reporter.
    pub fn progress(mut self, reporter: impl ProgressReporter + 'static) -> Self {
        self.progress = Some(Box::new(reporter));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overwrite_policy_default() {
        assert_eq!(OverwritePolicy::default(), OverwritePolicy::Error);
    }

    #[test]
    fn test_path_safety_default() {
        assert_eq!(PathSafety::default(), PathSafety::Strict);
    }

    #[test]
    fn test_link_policy_default() {
        assert_eq!(LinkPolicy::default(), LinkPolicy::Forbid);
    }

    #[test]
    fn test_threads_count() {
        use std::num::NonZeroUsize;
        assert_eq!(Threads::Single.count(), 1);
        assert_eq!(Threads::Count(NonZeroUsize::new(4).unwrap()).count(), 4);
        assert!(Threads::Auto.count() >= 1);
    }

    #[test]
    fn test_threads_count_or_single() {
        // count_or_single(0) should return Single
        assert_eq!(Threads::count_or_single(0), Threads::Single);
        // count_or_single(n) for n > 0 should return Count(n)
        assert_eq!(Threads::count_or_single(4).count(), 4);
        assert_eq!(Threads::count_or_single(1).count(), 1);
    }

    #[test]
    fn test_preserve_metadata() {
        let all = PreserveMetadata::all();
        assert!(all.modification_time);
        assert!(all.creation_time);
        assert!(all.attributes);

        let none = PreserveMetadata::none();
        assert!(!none.modification_time);
        assert!(!none.creation_time);
        assert!(!none.attributes);
    }

    #[test]
    fn test_preserve_metadata_times() {
        let times = PreserveMetadata::times();
        assert!(times.modification_time);
        assert!(times.creation_time);
        assert!(!times.attributes);
    }

    #[test]
    fn test_preserve_metadata_modification_time_only() {
        let mtime = PreserveMetadata::modification_time_only();
        assert!(mtime.modification_time);
        assert!(!mtime.creation_time);
        assert!(!mtime.attributes);
    }

    #[test]
    fn test_extract_options_builder() {
        let opts = ExtractOptions::new()
            .overwrite(OverwritePolicy::Skip)
            .path_safety(PathSafety::Relaxed)
            .threads(Threads::count_or_single(2));

        assert_eq!(opts.overwrite, OverwritePolicy::Skip);
        assert_eq!(opts.path_safety, PathSafety::Relaxed);
        assert_eq!(opts.threads.count(), 2);
    }

    #[test]
    fn test_filter_policy_default() {
        assert_eq!(FilterPolicy::default(), FilterPolicy::Include);
    }

    #[test]
    fn test_filter_policy_is_include_exclude() {
        assert!(FilterPolicy::Include.is_include());
        assert!(!FilterPolicy::Include.is_exclude());

        assert!(!FilterPolicy::Exclude.is_include());
        assert!(FilterPolicy::Exclude.is_exclude());
    }

    #[test]
    fn test_filter_policy_apply() {
        // Include policy: passes through the match result
        assert!(FilterPolicy::Include.apply(true));
        assert!(!FilterPolicy::Include.apply(false));

        // Exclude policy: inverts the match result
        assert!(!FilterPolicy::Exclude.apply(true));
        assert!(FilterPolicy::Exclude.apply(false));
    }

    #[test]
    fn test_threads_count_always_positive() {
        // Critical invariant: count() must always return >= 1
        use std::num::NonZeroUsize;

        // Single is always 1
        assert!(Threads::Single.count() >= 1);

        // Count with NonZeroUsize is always >= 1
        let count = Threads::Count(NonZeroUsize::new(1).unwrap());
        assert!(count.count() >= 1);

        // Count with larger values
        for n in [1, 2, 4, 8, 16, 100] {
            let threads = Threads::Count(NonZeroUsize::new(n).unwrap());
            assert!(threads.count() >= 1, "count() should always be >= 1");
            assert_eq!(threads.count(), n);
        }

        // Auto should always be >= 1
        assert!(Threads::Auto.count() >= 1);
    }

    #[test]
    fn test_threads_count_or_single_invariants() {
        // count_or_single should never produce a Threads that returns count() < 1
        for n in 0..=100 {
            let threads = Threads::count_or_single(n);
            assert!(
                threads.count() >= 1,
                "count_or_single({}) produced count() = {}",
                n,
                threads.count()
            );
        }
    }

    #[test]
    fn test_extract_options_clone_settings() {
        let original = ExtractOptions::new()
            .overwrite(OverwritePolicy::Skip)
            .path_safety(PathSafety::Relaxed)
            .link_policy(LinkPolicy::Allow)
            .threads(Threads::count_or_single(4));

        let cloned = original.clone_settings();

        // Verify all cloneable fields match
        assert_eq!(cloned.overwrite, OverwritePolicy::Skip);
        assert_eq!(cloned.path_safety, PathSafety::Relaxed);
        assert_eq!(cloned.link_policy, LinkPolicy::Allow);
        assert_eq!(cloned.threads.count(), 4);

        // Progress callback should be None in cloned version
        assert!(cloned.progress.is_none());
    }
}
