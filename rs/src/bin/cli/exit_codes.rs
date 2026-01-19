//! Exit codes for the CLI tool.

use zesven::Error;

/// Exit code constants
pub const SUCCESS: i32 = 0;
/// Operation completed with warnings
pub const WARNING: i32 = 1;
/// Fatal error occurred
pub const FATAL_ERROR: i32 = 2;
/// Archive format error
pub const BAD_ARCHIVE: i32 = 3;
/// Wrong password
pub const WRONG_PASSWORD: i32 = 4;
/// I/O error
pub const IO_ERROR: i32 = 5;
/// Ctrl+C (128 + SIGINT)
pub const USER_INTERRUPT: i32 = 130;
/// Invalid command line arguments
pub const BAD_ARGS: i32 = 255;

/// Exit code enum for structured handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // UserInterrupt reserved for signal handling
pub enum ExitCode {
    Success,
    Warning,
    FatalError,
    BadArchive,
    WrongPassword,
    IoError,
    UserInterrupt,
    BadArgs,
}

impl ExitCode {
    /// Returns the numeric exit code
    pub fn code(self) -> i32 {
        match self {
            Self::Success => SUCCESS,
            Self::Warning => WARNING,
            Self::FatalError => FATAL_ERROR,
            Self::BadArchive => BAD_ARCHIVE,
            Self::WrongPassword => WRONG_PASSWORD,
            Self::IoError => IO_ERROR,
            Self::UserInterrupt => USER_INTERRUPT,
            Self::BadArgs => BAD_ARGS,
        }
    }
}

/// Converts a zesven error to an exit code
pub fn error_to_exit_code(error: &Error) -> ExitCode {
    match error {
        Error::Io(_) => ExitCode::IoError,
        Error::InvalidFormat(_) | Error::CorruptHeader { .. } => ExitCode::BadArchive,
        Error::WrongPassword { .. } => ExitCode::WrongPassword,
        Error::CrcMismatch { .. } => ExitCode::BadArchive,
        Error::UnsupportedMethod { .. } => ExitCode::BadArchive,
        Error::UnsupportedFeature { .. } => ExitCode::BadArchive,
        Error::PathTraversal { .. } => ExitCode::FatalError,
        Error::ResourceLimitExceeded(_) => ExitCode::FatalError,
        Error::InvalidArchivePath(_) => ExitCode::BadArgs,
        Error::CryptoError(_) => ExitCode::FatalError,
        Error::VolumeMissing { .. } => ExitCode::IoError,
        Error::VolumeCorrupted { .. } => ExitCode::BadArchive,
        Error::IncompleteArchive { .. } => ExitCode::BadArchive,
        Error::Cancelled => ExitCode::UserInterrupt,
        #[cfg(feature = "regex")]
        Error::InvalidRegex { .. } => ExitCode::BadArgs,
        // Future error variants - required by #[non_exhaustive]
        _ => ExitCode::FatalError,
    }
}
