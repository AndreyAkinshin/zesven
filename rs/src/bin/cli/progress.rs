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

/// Progress bar for writing an archive.
///
/// Writing reports differently from extracting, and the bar has to follow. A
/// batch of entries is announced all at once, before any of it is compressed,
/// because they are compressed at the same time as each other - so counting
/// starts would jump the bar to the end and leave it there. What moves it is
/// entries reaching the archive.
///
/// Within a single large entry there is nothing to count in entries, so the
/// message carries what has been produced instead: one file can be the whole
/// archive and the wait is the whole run.
#[derive(Clone)]
pub struct WriteProgress {
    /// Cloning shares the bar rather than drawing a second one: the writer is
    /// given one handle and this side keeps another.
    bar: ProgressBar,
    /// What is being worked on, in the order it was announced.
    ///
    /// A batch names all of its entries before any of them is done, so the
    /// message says how many are in hand rather than pretending one of them is
    /// the current one.
    in_hand: Arc<Mutex<Vec<String>>>,
    quiet: bool,
}

impl WriteProgress {
    pub fn new(total_entries: u64, quiet: bool) -> Self {
        let bar = if quiet {
            ProgressBar::hidden()
        } else {
            let pb = ProgressBar::new(total_entries);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}]                          {pos}/{len} {wide_msg}",
                    )
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb
        };

        Self {
            bar,
            in_hand: Arc::new(Mutex::new(Vec::new())),
            quiet,
        }
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }

    pub fn finish_with_message(&self, msg: &str) {
        self.bar.abandon_with_message(msg.to_string());
    }

    /// Says what is being worked on, in as few words as the terminal allows.
    fn describe(&self) {
        if self.quiet {
            return;
        }
        let Ok(names) = self.in_hand.lock() else {
            return;
        };
        let message = match names.len() {
            0 => String::new(),
            1 => format!("compressing {}", shorten(&names[0])),
            n => format!("compressing {n} entries"),
        };
        self.bar.set_message(message);
    }
}

/// Trims a path from the left, which is where the uninteresting part is.
fn shorten(name: &str) -> String {
    if name.chars().count() > 40 {
        let tail: String = name.chars().skip(name.chars().count() - 37).collect();
        format!("...{tail}")
    } else {
        name.to_string()
    }
}

impl ProgressReporter for WriteProgress {
    fn on_entry_start(&mut self, entry_name: &str, _size: u64) {
        if let Ok(mut names) = self.in_hand.lock() {
            names.push(entry_name.to_string());
        }
        self.describe();
    }

    fn on_progress(&mut self, produced: u64, declared: u64) -> bool {
        if self.quiet {
            return true;
        }
        // Only a large entry reports this, and it is the one case where the
        // entry count says nothing for minutes at a time.
        if let Ok(names) = self.in_hand.lock()
            && let Some(name) = names.first()
        {
            self.bar.set_message(format!(
                "compressing {} - {} written of about {}",
                shorten(name),
                indicatif::HumanBytes(produced),
                indicatif::HumanBytes(declared),
            ));
        }
        true
    }

    fn on_entry_complete(&mut self, entry_name: &str, _success: bool) {
        if let Ok(mut names) = self.in_hand.lock() {
            names.retain(|held| held != entry_name);
        }
        self.bar.inc(1);
        self.describe();
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

#[cfg(test)]
mod tests {
    use super::WriteProgress;
    use zesven::progress::ProgressReporter;

    /// The bar follows entries reaching the archive, not entries announced.
    ///
    /// A batch names everything it holds before any of it is compressed, so a
    /// bar that counted announcements would jump to the end and sit there for
    /// the whole wait - which is the shape this exists to stop showing.
    #[test]
    fn test_the_bar_follows_what_has_been_written() {
        let mut bar = WriteProgress::new(3, true);

        bar.on_entry_start("a.bin", 10);
        bar.on_entry_start("b.bin", 10);
        bar.on_entry_start("c.bin", 10);
        assert_eq!(
            bar.bar.position(),
            0,
            "announcing a batch moved the bar before anything was written",
        );

        bar.on_entry_complete("a.bin", true);
        assert_eq!(bar.bar.position(), 1);
        bar.on_entry_complete("b.bin", true);
        bar.on_entry_complete("c.bin", true);
        assert_eq!(bar.bar.position(), 3);
    }

    /// What is in hand is what has been announced and not yet finished.
    #[test]
    fn test_the_message_tracks_what_is_being_worked_on() {
        let mut bar = WriteProgress::new(2, true);

        bar.on_entry_start("first.bin", 10);
        bar.on_entry_start("second.bin", 10);
        assert_eq!(bar.in_hand.lock().expect("held").len(), 2);

        bar.on_entry_complete("first.bin", true);
        let held = bar.in_hand.lock().expect("held");
        assert_eq!(*held, vec!["second.bin".to_string()]);
    }
}
