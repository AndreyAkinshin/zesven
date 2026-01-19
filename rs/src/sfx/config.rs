//! SFX archive configuration types.

use std::path::PathBuf;

/// Configuration for self-extracting archive behavior.
///
/// These settings control how the SFX executable behaves when run,
/// including extraction path, post-extraction commands, and UI options.
#[derive(Debug, Clone, Default)]
pub struct SfxConfig {
    /// Title shown in the extraction dialog.
    pub title: Option<String>,

    /// Default extraction path.
    /// If None, extracts to a temporary directory or prompts user.
    pub extract_path: Option<PathBuf>,

    /// Program to run after extraction completes.
    pub run_program: Option<String>,

    /// Parameters to pass to the run_program.
    pub run_parameters: Option<String>,

    /// Whether to show a progress dialog during extraction.
    pub progress: bool,

    /// Prompt message shown before extraction begins.
    pub begin_prompt: Option<String>,

    /// Icon data for the SFX executable (Windows only, ICO format).
    pub icon: Option<Vec<u8>>,

    /// Whether to delete files after running the program.
    pub delete_after_run: bool,

    /// Directory to extract to (overrides extract_path if set).
    pub install_path: Option<String>,

    /// Error title shown in error dialogs.
    pub error_title: Option<String>,
}

impl SfxConfig {
    /// Creates a new empty SFX configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the title shown in extraction dialogs.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the default extraction path.
    pub fn extract_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.extract_path = Some(path.into());
        self
    }

    /// Sets the program to run after extraction.
    pub fn run_program(mut self, program: impl Into<String>) -> Self {
        self.run_program = Some(program.into());
        self
    }

    /// Sets parameters for the post-extraction program.
    pub fn run_parameters(mut self, params: impl Into<String>) -> Self {
        self.run_parameters = Some(params.into());
        self
    }

    /// Enables or disables the progress dialog.
    pub fn progress(mut self, show: bool) -> Self {
        self.progress = show;
        self
    }

    /// Sets the message shown before extraction begins.
    pub fn begin_prompt(mut self, message: impl Into<String>) -> Self {
        self.begin_prompt = Some(message.into());
        self
    }

    /// Sets the icon for the SFX executable (Windows only).
    pub fn icon(mut self, ico_data: Vec<u8>) -> Self {
        self.icon = Some(ico_data);
        self
    }

    /// Returns true if any configuration settings are set.
    pub fn has_settings(&self) -> bool {
        self.title.is_some()
            || self.extract_path.is_some()
            || self.run_program.is_some()
            || self.run_parameters.is_some()
            || self.progress
            || self.begin_prompt.is_some()
            || self.delete_after_run
            || self.install_path.is_some()
    }

    /// Encodes the configuration into 7-Zip SFX config format.
    ///
    /// Format: `;!@Install@!UTF-8!` marker followed by key=value pairs.
    pub fn encode(&self) -> Vec<u8> {
        if !self.has_settings() {
            return Vec::new();
        }

        let mut config = String::new();
        config.push_str(";!@Install@!UTF-8!\n");

        if let Some(ref title) = self.title {
            config.push_str(&format!("Title=\"{}\"\n", escape_value(title)));
        }

        if let Some(ref begin_prompt) = self.begin_prompt {
            config.push_str(&format!("BeginPrompt=\"{}\"\n", escape_value(begin_prompt)));
        }

        if self.progress {
            config.push_str("Progress=\"yes\"\n");
        }

        if let Some(ref run_program) = self.run_program {
            config.push_str(&format!("RunProgram=\"{}\"\n", escape_value(run_program)));
        }

        if let Some(ref install_path) = self.install_path {
            config.push_str(&format!("InstallPath=\"{}\"\n", escape_value(install_path)));
        }

        if let Some(ref extract_path) = self.extract_path {
            config.push_str(&format!(
                "Directory=\"{}\"\n",
                escape_value(&extract_path.display().to_string())
            ));
        }

        if self.delete_after_run {
            config.push_str("Delete=\"$OUTDIR\"\n");
        }

        if let Some(ref error_title) = self.error_title {
            config.push_str(&format!("ErrorTitle=\"{}\"\n", escape_value(error_title)));
        }

        config.push_str(";!@InstallEnd@!\n");

        config.into_bytes()
    }
}

/// Escapes special characters in config values.
fn escape_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config() {
        let config = SfxConfig::new();
        assert!(!config.has_settings());
        assert!(config.encode().is_empty());
    }

    #[test]
    fn test_config_with_title() {
        let config = SfxConfig::new().title("My Installer");
        assert!(config.has_settings());
        let encoded = String::from_utf8(config.encode()).unwrap();
        assert!(encoded.contains(";!@Install@!UTF-8!"));
        assert!(encoded.contains("Title=\"My Installer\""));
        assert!(encoded.contains(";!@InstallEnd@!"));
    }

    #[test]
    fn test_full_config() {
        let config = SfxConfig::new()
            .title("Test App")
            .run_program("setup.exe")
            .run_parameters("/silent")
            .progress(true)
            .begin_prompt("Install Test App?");

        let encoded = String::from_utf8(config.encode()).unwrap();
        assert!(encoded.contains("Title=\"Test App\""));
        assert!(encoded.contains("RunProgram=\"setup.exe\""));
        assert!(encoded.contains("Progress=\"yes\""));
        assert!(encoded.contains("BeginPrompt=\"Install Test App?\""));
    }

    #[test]
    fn test_escape_special_chars() {
        let config = SfxConfig::new().title("Test \"App\" with\\backslash");
        let encoded = String::from_utf8(config.encode()).unwrap();
        assert!(encoded.contains("Title=\"Test \\\"App\\\" with\\\\backslash\""));
    }

    #[test]
    fn test_builder_pattern() {
        let config = SfxConfig::new()
            .title("My App")
            .extract_path("/tmp/myapp")
            .run_program("./install.sh")
            .progress(true);

        assert_eq!(config.title, Some("My App".to_string()));
        assert_eq!(config.extract_path, Some(PathBuf::from("/tmp/myapp")));
        assert_eq!(config.run_program, Some("./install.sh".to_string()));
        assert!(config.progress);
    }
}
