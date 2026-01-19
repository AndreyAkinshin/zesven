//! Async-specific options for archive operations.
//!
//! This module provides async-aware options that extend the base options
//! with cancellation support and async callbacks.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::format::streams::ResourceLimits;
use crate::read::{LinkPolicy, OverwritePolicy, PathSafety, PreserveMetadata, Threads};

#[cfg(feature = "aes")]
use crate::async_password::AsyncPasswordProvider;

/// Async progress callback for extraction operations.
///
/// Unlike the sync `ProgressReporter`, this trait allows for async operations
/// in the callbacks, such as updating a UI or sending progress over a channel.
pub trait AsyncProgressCallback: Send + Sync {
    /// Called when extraction of an entry begins.
    fn on_entry_start(
        &self,
        entry_name: &str,
        entry_size: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Called periodically during extraction.
    fn on_progress(
        &self,
        bytes_extracted: u64,
        total_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Called when extraction of an entry completes.
    fn on_entry_complete(
        &self,
        entry_name: &str,
        success: bool,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Options for async extraction operations.
///
/// This struct extends `ExtractOptions` with async-specific features like
/// cancellation tokens and async progress callbacks.
pub struct AsyncExtractOptions {
    /// Policy for handling existing files.
    pub overwrite: OverwritePolicy,
    /// Path safety validation policy.
    pub path_safety: PathSafety,
    /// Symbolic link handling policy.
    pub link_policy: LinkPolicy,
    /// Resource limits for extraction.
    pub limits: ResourceLimits,
    /// Thread configuration (for CPU-bound decompression).
    pub threads: Threads,
    /// Metadata preservation options.
    pub preserve_metadata: PreserveMetadata,
    /// Cancellation token for graceful cancellation.
    pub cancel_token: Option<CancellationToken>,
    /// Password provider for encrypted archives.
    #[cfg(feature = "aes")]
    pub password_provider: Option<Arc<dyn AsyncPasswordProvider>>,
    /// Async progress callback (optional).
    pub progress: Option<Arc<dyn AsyncProgressCallback>>,
}

#[allow(clippy::derivable_impls)] // Manual impl needed due to #[cfg] conditional field
impl Default for AsyncExtractOptions {
    fn default() -> Self {
        Self {
            overwrite: OverwritePolicy::default(),
            path_safety: PathSafety::default(),
            link_policy: LinkPolicy::default(),
            limits: ResourceLimits::default(),
            threads: Threads::default(),
            preserve_metadata: PreserveMetadata::default(),
            cancel_token: None,
            #[cfg(feature = "aes")]
            password_provider: None,
            progress: None,
        }
    }
}

impl std::fmt::Debug for AsyncExtractOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncExtractOptions")
            .field("overwrite", &self.overwrite)
            .field("path_safety", &self.path_safety)
            .field("link_policy", &self.link_policy)
            .field("threads", &self.threads)
            .field("preserve_metadata", &self.preserve_metadata)
            .field("has_cancel_token", &self.cancel_token.is_some())
            .finish_non_exhaustive()
    }
}

impl AsyncExtractOptions {
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

    /// Sets the cancellation token for graceful cancellation.
    pub fn cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Sets the async password provider for encrypted archives.
    #[cfg(feature = "aes")]
    pub fn password_provider(mut self, provider: Arc<dyn AsyncPasswordProvider>) -> Self {
        self.password_provider = Some(provider);
        self
    }

    /// Sets the async progress callback.
    pub fn progress(mut self, callback: Arc<dyn AsyncProgressCallback>) -> Self {
        self.progress = Some(callback);
        self
    }

    /// Returns true if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    }
}

/// Options for async test (integrity verification) operations.
pub struct AsyncTestOptions {
    /// Resource limits for testing.
    pub limits: ResourceLimits,
    /// Thread configuration.
    pub threads: Threads,
    /// Cancellation token for graceful cancellation.
    pub cancel_token: Option<CancellationToken>,
    /// Password provider for encrypted archives.
    #[cfg(feature = "aes")]
    pub password_provider: Option<Arc<dyn AsyncPasswordProvider>>,
    /// Async progress callback (optional).
    pub progress: Option<Arc<dyn AsyncProgressCallback>>,
}

#[allow(clippy::derivable_impls)] // Manual impl needed due to #[cfg] conditional field
impl Default for AsyncTestOptions {
    fn default() -> Self {
        Self {
            limits: ResourceLimits::default(),
            threads: Threads::default(),
            cancel_token: None,
            #[cfg(feature = "aes")]
            password_provider: None,
            progress: None,
        }
    }
}

impl std::fmt::Debug for AsyncTestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncTestOptions")
            .field("threads", &self.threads)
            .field("has_cancel_token", &self.cancel_token.is_some())
            .finish_non_exhaustive()
    }
}

impl AsyncTestOptions {
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

    /// Sets the cancellation token for graceful cancellation.
    pub fn cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Sets the async password provider for encrypted archives.
    #[cfg(feature = "aes")]
    pub fn password_provider(mut self, provider: Arc<dyn AsyncPasswordProvider>) -> Self {
        self.password_provider = Some(provider);
        self
    }

    /// Sets the async progress callback.
    pub fn progress(mut self, callback: Arc<dyn AsyncProgressCallback>) -> Self {
        self.progress = Some(callback);
        self
    }

    /// Returns true if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    }
}

/// A simple channel-based progress reporter for async contexts.
///
/// This implementation sends progress updates through a tokio channel.
pub struct ChannelProgressReporter {
    sender: tokio::sync::mpsc::Sender<ProgressEvent>,
}

/// Progress events sent by `ChannelProgressReporter`.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Extraction of an entry has started.
    EntryStart {
        /// The entry name.
        name: String,
        /// The entry size in bytes.
        size: u64,
    },
    /// Progress update during extraction.
    Progress {
        /// Bytes extracted so far.
        bytes_extracted: u64,
        /// Total bytes to extract.
        total_bytes: u64,
    },
    /// Extraction of an entry has completed.
    EntryComplete {
        /// The entry name.
        name: String,
        /// Whether extraction was successful.
        success: bool,
    },
}

impl ChannelProgressReporter {
    /// Creates a new channel-based progress reporter.
    ///
    /// Returns a tuple of (reporter, receiver). The receiver can be used
    /// to receive progress events.
    pub fn new(buffer_size: usize) -> (Self, tokio::sync::mpsc::Receiver<ProgressEvent>) {
        let (tx, rx) = tokio::sync::mpsc::channel(buffer_size);
        (Self { sender: tx }, rx)
    }
}

impl AsyncProgressCallback for ChannelProgressReporter {
    fn on_entry_start(
        &self,
        entry_name: &str,
        entry_size: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let event = ProgressEvent::EntryStart {
            name: entry_name.to_string(),
            size: entry_size,
        };
        Box::pin(async move {
            let _ = self.sender.send(event).await;
        })
    }

    fn on_progress(
        &self,
        bytes_extracted: u64,
        total_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let event = ProgressEvent::Progress {
            bytes_extracted,
            total_bytes,
        };
        Box::pin(async move {
            let _ = self.sender.send(event).await;
        })
    }

    fn on_entry_complete(
        &self,
        entry_name: &str,
        success: bool,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let event = ProgressEvent::EntryComplete {
            name: entry_name.to_string(),
            success,
        };
        Box::pin(async move {
            let _ = self.sender.send(event).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_async_extract_options_default() {
        let opts = AsyncExtractOptions::default();
        assert_eq!(opts.overwrite, OverwritePolicy::Error);
        assert_eq!(opts.path_safety, PathSafety::Strict);
        assert!(!opts.is_cancelled());
    }

    #[test]
    fn test_async_extract_options_builder() {
        let token = CancellationToken::new();
        let opts = AsyncExtractOptions::new()
            .overwrite(OverwritePolicy::Skip)
            .path_safety(PathSafety::Relaxed)
            .threads(Threads::count_or_single(4))
            .cancel_token(token.clone());

        assert_eq!(opts.overwrite, OverwritePolicy::Skip);
        assert_eq!(opts.path_safety, PathSafety::Relaxed);
        assert_eq!(opts.threads.count(), 4);
        assert!(!opts.is_cancelled());

        token.cancel();
        assert!(opts.is_cancelled());
    }

    #[test]
    fn test_async_test_options_default() {
        let opts = AsyncTestOptions::default();
        assert!(!opts.is_cancelled());
    }

    #[tokio::test]
    async fn test_channel_progress_reporter() {
        let (reporter, mut rx) = ChannelProgressReporter::new(10);

        reporter.on_entry_start("test.txt", 100).await;
        reporter.on_progress(50, 100).await;
        reporter.on_entry_complete("test.txt", true).await;

        let event1 = rx.recv().await.unwrap();
        assert!(matches!(event1, ProgressEvent::EntryStart { .. }));

        let event2 = rx.recv().await.unwrap();
        assert!(matches!(event2, ProgressEvent::Progress { .. }));

        let event3 = rx.recv().await.unwrap();
        assert!(matches!(event3, ProgressEvent::EntryComplete { .. }));
    }
}
