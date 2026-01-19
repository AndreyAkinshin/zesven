//! Progress bar implementation for CLI operations.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::{Arc, Mutex};
use zesven::progress::ProgressReporter;

/// Progress display for CLI operations
pub struct CliProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    current: Arc<Mutex<Option<ProgressBar>>>,
    quiet: bool,
}

impl CliProgress {
    /// Creates a new progress display
    pub fn new(total_entries: u64, quiet: bool) -> Self {
        let multi = MultiProgress::new();

        let overall = if quiet {
            ProgressBar::hidden()
        } else {
            let pb = multi.add(ProgressBar::new(total_entries));
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} files ({eta})")
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb
        };

        Self {
            multi,
            overall,
            current: Arc::new(Mutex::new(None)),
            quiet,
        }
    }

    /// Sets a message on the overall progress bar
    pub fn set_message(&self, msg: impl Into<String>) {
        if !self.quiet {
            self.overall.set_message(msg.into());
        }
    }

    /// Increments the overall progress
    #[allow(dead_code)] // Part of progress API
    pub fn inc(&self, delta: u64) {
        self.overall.inc(delta);
    }

    /// Finishes the progress display
    pub fn finish(&self) {
        self.overall.finish_with_message("Done");
    }

    /// Finishes with a custom message
    pub fn finish_with_message(&self, msg: impl Into<String>) {
        self.overall.finish_with_message(msg.into());
    }

    /// Creates a spinner for an indeterminate operation
    #[allow(dead_code)] // Part of progress API
    pub fn create_spinner(&self, message: &str) -> ProgressBar {
        if self.quiet {
            return ProgressBar::hidden();
        }

        let pb = self.multi.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message(message.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    }
}

impl ProgressReporter for CliProgress {
    fn on_entry_start(&mut self, entry_name: &str, entry_size: u64) {
        if self.quiet {
            return;
        }

        let pb = self.multi.add(ProgressBar::new(entry_size));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  {spinner:.green} {wide_msg} [{bar:30}] {bytes}/{total_bytes}")
                .unwrap()
                .progress_chars("#>-"),
        );

        // Truncate long names
        let display_name = if entry_name.len() > 40 {
            format!("...{}", &entry_name[entry_name.len() - 37..])
        } else {
            entry_name.to_string()
        };
        pb.set_message(display_name);

        *self.current.lock().unwrap() = Some(pb);
    }

    fn on_progress(&mut self, bytes_extracted: u64, _total_bytes: u64) -> bool {
        if let Some(pb) = self.current.lock().unwrap().as_ref() {
            pb.set_position(bytes_extracted);
        }
        true // Continue extraction
    }

    fn on_entry_complete(&mut self, _entry_name: &str, success: bool) {
        if let Some(pb) = self.current.lock().unwrap().take() {
            if success {
                pb.finish_and_clear();
            } else {
                pb.abandon_with_message("Error");
            }
        }
        self.overall.inc(1);
    }
}

/// Simple progress bar for single operations
pub struct SimpleProgress {
    bar: ProgressBar,
}

impl SimpleProgress {
    /// Creates a new simple progress bar
    pub fn new(total: u64, quiet: bool) -> Self {
        let bar = if quiet {
            ProgressBar::hidden()
        } else {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb
        };

        Self { bar }
    }

    /// Sets the current position
    #[allow(dead_code)] // Part of progress API
    pub fn set_position(&self, pos: u64) {
        self.bar.set_position(pos);
    }

    /// Increments the progress
    pub fn inc(&self, delta: u64) {
        self.bar.inc(delta);
    }

    /// Sets the message
    pub fn set_message(&self, msg: impl Into<String>) {
        self.bar.set_message(msg.into());
    }

    /// Finishes the progress bar
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }

    /// Finishes with a message
    pub fn finish_with_message(&self, msg: impl Into<String>) {
        self.bar.finish_with_message(msg.into());
    }
}
