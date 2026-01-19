//! Output formatting for CLI operations.

use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use zesven::read::{ArchiveInfo, Entry, ExtractResult, TestResult};

/// Trait for output formatting
pub trait OutputFormatter {
    /// Formats a list of entries
    fn format_list(&self, entries: &[Entry], technical: bool) -> String;

    /// Formats archive information
    fn format_info(&self, info: &ArchiveInfo) -> String;

    /// Formats extraction results
    fn format_extract_result(&self, result: &ExtractResult) -> String;

    /// Formats test results
    fn format_test_result(&self, result: &TestResult) -> String;
}

/// Human-readable output formatter
pub struct HumanFormatter;

impl OutputFormatter for HumanFormatter {
    fn format_list(&self, entries: &[Entry], technical: bool) -> String {
        let mut output = String::new();

        // Header
        if technical {
            output.push_str(&format!(
                "{:>12} {:>12} {:>19} {:>10} {}\n",
                "Size", "Packed", "Modified", "CRC", "Name"
            ));
        } else {
            output.push_str(&format!("{:>12} {:>19} {}\n", "Size", "Modified", "Name"));
        }
        output.push_str(&"-".repeat(70));
        output.push('\n');

        let mut total_size: u64 = 0;
        let mut file_count = 0;
        let mut dir_count = 0;

        for entry in entries {
            if entry.is_directory {
                dir_count += 1;
            } else {
                file_count += 1;
                total_size += entry.size;
            }

            let size_str = if entry.is_directory {
                String::new()
            } else {
                humanize_bytes(entry.size)
            };

            let mtime_str = entry
                .modified()
                .map(format_timestamp)
                .unwrap_or_else(|| "-".to_string());

            let type_indicator = if entry.is_directory { "D" } else { "" };

            if technical {
                let crc_str = entry
                    .crc32
                    .map(|c| format!("{:08X}", c))
                    .unwrap_or_else(|| "-".to_string());

                output.push_str(&format!(
                    "{:>12} {:>12} {:>19} {:>10} {}{}\n",
                    size_str,
                    "-", // packed size not easily available
                    mtime_str,
                    crc_str,
                    entry.path.as_str(),
                    type_indicator
                ));
            } else {
                output.push_str(&format!(
                    "{:>12} {:>19} {}{}\n",
                    size_str,
                    mtime_str,
                    entry.path.as_str(),
                    type_indicator
                ));
            }
        }

        // Footer
        output.push_str(&"-".repeat(70));
        output.push('\n');
        output.push_str(&format!(
            "{} files, {} directories, {} total\n",
            file_count,
            dir_count,
            humanize_bytes(total_size)
        ));

        output
    }

    fn format_info(&self, info: &ArchiveInfo) -> String {
        let mut output = String::new();

        output.push_str("Archive Information:\n");
        output.push_str(&"-".repeat(40));
        output.push('\n');
        output.push_str(&format!("  Entries:        {}\n", info.entry_count));
        output.push_str(&format!(
            "  Total size:     {}\n",
            humanize_bytes(info.total_size)
        ));
        output.push_str(&format!(
            "  Packed size:    {}\n",
            humanize_bytes(info.packed_size)
        ));
        output.push_str(&format!(
            "  Ratio:          {:.1}%\n",
            info.compression_ratio() * 100.0
        ));
        output.push_str(&format!(
            "  Space savings:  {:.1}%\n",
            info.space_savings() * 100.0
        ));
        output.push_str(&format!(
            "  Solid:          {}\n",
            if info.is_solid { "Yes" } else { "No" }
        ));
        output.push_str(&format!("  Folders:        {}\n", info.folder_count));

        if !info.compression_methods.is_empty() {
            let methods: Vec<_> = info
                .compression_methods
                .iter()
                .map(|m| format!("{:?}", m))
                .collect();
            output.push_str(&format!("  Methods:        {}\n", methods.join(", ")));
        }

        if info.has_encrypted_entries {
            output.push_str("  Encrypted:      Yes\n");
        }
        if info.has_encrypted_header {
            output.push_str("  Header enc.:    Yes\n");
        }

        output
    }

    fn format_extract_result(&self, result: &ExtractResult) -> String {
        let mut output = String::new();

        if result.is_ok() {
            output.push_str(&format!(
                "Extracted {} files ({} bytes)\n",
                result.entries_extracted,
                humanize_bytes(result.bytes_extracted)
            ));
            if result.entries_skipped > 0 {
                output.push_str(&format!("Skipped {} files\n", result.entries_skipped));
            }
        } else {
            output.push_str("Extraction completed with errors:\n");
            output.push_str(&format!("  Extracted: {}\n", result.entries_extracted));
            output.push_str(&format!("  Skipped:   {}\n", result.entries_skipped));
            output.push_str(&format!("  Failed:    {}\n", result.entries_failed));

            if !result.failures.is_empty() {
                output.push_str("\nFailures:\n");
                for (path, error) in &result.failures {
                    output.push_str(&format!("  {}: {}\n", path, error));
                }
            }
        }

        output
    }

    fn format_test_result(&self, result: &TestResult) -> String {
        let mut output = String::new();

        if result.is_ok() {
            output.push_str(&format!(
                "OK - {} files tested, all passed\n",
                result.entries_tested
            ));
        } else {
            output.push_str("Test completed with errors:\n");
            output.push_str(&format!("  Tested: {}\n", result.entries_tested));
            output.push_str(&format!("  Passed: {}\n", result.entries_passed));
            output.push_str(&format!("  Failed: {}\n", result.entries_failed));

            if !result.failures.is_empty() {
                output.push_str("\nFailures:\n");
                for (path, error) in &result.failures {
                    output.push_str(&format!("  {}: {}\n", path, error));
                }
            }
        }

        output
    }
}

/// JSON output formatter
pub struct JsonFormatter;

impl OutputFormatter for JsonFormatter {
    fn format_list(&self, entries: &[Entry], _technical: bool) -> String {
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                json!({
                    "path": e.path.as_str(),
                    "size": e.size,
                    "modified": e.modified().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
                    "crc32": e.crc32,
                    "is_directory": e.is_directory,
                    "encrypted": e.is_encrypted,
                })
            })
            .collect();

        serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
    }

    fn format_info(&self, info: &ArchiveInfo) -> String {
        let obj = json!({
            "entry_count": info.entry_count,
            "total_size": info.total_size,
            "packed_size": info.packed_size,
            "compression_ratio": info.compression_ratio(),
            "space_savings": info.space_savings(),
            "is_solid": info.is_solid,
            "folder_count": info.folder_count,
            "has_encrypted_entries": info.has_encrypted_entries,
            "has_encrypted_header": info.has_encrypted_header,
            "compression_methods": info.compression_methods.iter().map(|m| format!("{:?}", m)).collect::<Vec<_>>(),
        });

        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
    }

    fn format_extract_result(&self, result: &ExtractResult) -> String {
        let obj = json!({
            "success": result.is_ok(),
            "entries_extracted": result.entries_extracted,
            "entries_skipped": result.entries_skipped,
            "entries_failed": result.entries_failed,
            "bytes_extracted": result.bytes_extracted,
            "failures": result.failures.iter().map(|(p, e)| json!({"path": p, "error": e})).collect::<Vec<_>>(),
        });

        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
    }

    fn format_test_result(&self, result: &TestResult) -> String {
        let obj = json!({
            "success": result.is_ok(),
            "entries_tested": result.entries_tested,
            "entries_passed": result.entries_passed,
            "entries_failed": result.entries_failed,
            "failures": result.failures.iter().map(|(p, e)| json!({"path": p, "error": e})).collect::<Vec<_>>(),
        });

        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Creates the appropriate formatter based on output format
pub fn create_formatter(format: super::OutputFormat) -> Box<dyn OutputFormatter> {
    match format {
        super::OutputFormat::Human => Box::new(HumanFormatter),
        super::OutputFormat::Json => Box::new(JsonFormatter),
    }
}

/// Converts bytes to a human-readable string
pub fn humanize_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Formats a SystemTime as a datetime string
pub fn format_timestamp(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            // Simple formatting - in a real app you'd use chrono
            let secs = duration.as_secs();
            let days_since_epoch = secs / 86400;
            let time_of_day = secs % 86400;
            let hours = time_of_day / 3600;
            let minutes = (time_of_day % 3600) / 60;
            let seconds = time_of_day % 60;

            // Approximate date calculation
            let mut year = 1970;
            let mut remaining_days = days_since_epoch as i64;

            loop {
                let days_in_year = if is_leap_year(year) { 366 } else { 365 };
                if remaining_days < days_in_year {
                    break;
                }
                remaining_days -= days_in_year;
                year += 1;
            }

            let (month, day) = days_to_month_day(remaining_days as u32, is_leap_year(year));

            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                year, month, day, hours, minutes, seconds
            )
        }
        Err(_) => "-".to_string(),
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_to_month_day(day_of_year: u32, leap: bool) -> (u32, u32) {
    let days_in_months: [u32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut remaining = day_of_year;
    for (i, &days) in days_in_months.iter().enumerate() {
        if remaining < days {
            return (i as u32 + 1, remaining + 1);
        }
        remaining -= days;
    }

    (12, 31) // Fallback
}
