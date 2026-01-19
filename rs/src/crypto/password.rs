//! Password handling for 7z encryption.

use zeroize::Zeroizing;

/// A password for archive encryption/decryption.
///
/// This type stores the password securely and provides conversion to UTF-16LE
/// as required by 7z's key derivation function.
#[derive(Clone)]
pub struct Password {
    inner: Zeroizing<String>,
}

impl Password {
    /// Creates a new password from a string.
    pub fn new<S: Into<String>>(password: S) -> Self {
        Self {
            inner: Zeroizing::new(password.into()),
        }
    }

    /// Returns the password as UTF-16LE bytes for key derivation.
    ///
    /// 7z uses UTF-16LE encoding for passwords in its key derivation function.
    pub fn as_utf16_le(&self) -> Vec<u8> {
        self.inner
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect()
    }

    /// Returns the password as a string slice.
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Returns true if the password is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the length of the password in characters.
    pub fn len(&self) -> usize {
        self.inner.chars().count()
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't expose the actual password in debug output
        f.debug_struct("Password")
            .field("len", &self.inner.len())
            .finish()
    }
}

impl From<&str> for Password {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Password {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_utf16le_ascii() {
        let password = Password::new("test");
        let bytes = password.as_utf16_le();
        // "test" in UTF-16LE: t(0x74 0x00) e(0x65 0x00) s(0x73 0x00) t(0x74 0x00)
        assert_eq!(bytes, vec![0x74, 0x00, 0x65, 0x00, 0x73, 0x00, 0x74, 0x00]);
    }

    #[test]
    fn test_password_utf16le_unicode() {
        let password = Password::new("пароль"); // Russian word for "password"
        let bytes = password.as_utf16_le();
        // Each Cyrillic character uses 2 bytes in UTF-16LE
        assert_eq!(bytes.len(), 12); // 6 characters * 2 bytes
    }

    #[test]
    fn test_password_utf16le_empty() {
        let password = Password::new("");
        let bytes = password.as_utf16_le();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_password_debug() {
        let password = Password::new("secret");
        let debug = format!("{:?}", password);
        // Debug output should not contain the actual password
        assert!(!debug.contains("secret"));
        assert!(debug.contains("len"));
    }

    #[test]
    fn test_password_from_str() {
        let password: Password = "test".into();
        assert_eq!(password.as_str(), "test");
    }

    #[test]
    fn test_password_from_string() {
        let password: Password = String::from("test").into();
        assert_eq!(password.as_str(), "test");
    }

    #[test]
    fn test_password_len() {
        let password = Password::new("test");
        assert_eq!(password.len(), 4);
        assert!(!password.is_empty());

        let empty = Password::new("");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }
}
