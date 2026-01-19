//! Async password handling for 7z encryption.
//!
//! This module provides async password providers for non-blocking password
//! requests during archive operations.

use std::future::Future;
use std::pin::Pin;

#[cfg(feature = "aes")]
use crate::Password;

/// Async password provider trait for non-blocking password requests.
///
/// This trait allows implementing custom async password providers that can
/// prompt users for passwords without blocking the async runtime.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::async_password::AsyncPasswordProvider;
///
/// struct MyPasswordProvider {
///     password: String,
/// }
///
/// impl AsyncPasswordProvider for MyPasswordProvider {
///     fn get_password(&self) -> Pin<Box<dyn Future<Output = Option<Password>> + Send + '_>> {
///         Box::pin(async move {
///             Some(Password::new(&self.password))
///         })
///     }
/// }
/// ```
#[cfg(feature = "aes")]
pub trait AsyncPasswordProvider: Send + Sync {
    /// Asynchronously retrieves a password.
    ///
    /// Returns `Some(Password)` if a password is available, or `None` if
    /// no password is provided (e.g., user cancelled).
    fn get_password(&self) -> Pin<Box<dyn Future<Output = Option<Password>> + Send + '_>>;
}

/// A simple async password wrapper that returns a pre-configured password.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::async_password::AsyncPassword;
///
/// let provider = AsyncPassword::new("my_secret_password");
/// ```
#[cfg(feature = "aes")]
#[derive(Debug, Clone)]
pub struct AsyncPassword {
    password: Option<Password>,
}

#[cfg(feature = "aes")]
impl AsyncPassword {
    /// Creates a new async password provider with the given password.
    pub fn new(password: impl Into<Password>) -> Self {
        Self {
            password: Some(password.into()),
        }
    }

    /// Creates an async password provider with no password.
    pub fn none() -> Self {
        Self { password: None }
    }

    /// Creates an async password provider from an optional password.
    pub fn from_option(password: Option<Password>) -> Self {
        Self { password }
    }
}

#[cfg(feature = "aes")]
impl AsyncPasswordProvider for AsyncPassword {
    fn get_password(&self) -> Pin<Box<dyn Future<Output = Option<Password>> + Send + '_>> {
        let password = self.password.clone();
        Box::pin(async move { password })
    }
}

/// Channel-based password provider for interactive use.
///
/// This provider waits for a password to be sent via a oneshot channel,
/// allowing for interactive password prompts in async contexts.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::async_password::InteractivePasswordProvider;
/// use tokio::sync::oneshot;
///
/// let (tx, provider) = InteractivePasswordProvider::new();
///
/// // In another task, prompt the user and send the password
/// tokio::spawn(async move {
///     // Get password from user somehow...
///     let password = Password::new("user_entered_password");
///     tx.send(Some(password)).ok();
/// });
///
/// // The provider will wait for the password
/// let password = provider.get_password().await;
/// ```
#[cfg(feature = "aes")]
pub struct InteractivePasswordProvider {
    receiver: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Option<Password>>>>,
}

#[cfg(feature = "aes")]
impl InteractivePasswordProvider {
    /// Creates a new interactive password provider.
    ///
    /// Returns a tuple of (sender, provider). The sender can be used to
    /// send the password when it becomes available.
    pub fn new() -> (tokio::sync::oneshot::Sender<Option<Password>>, Self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let provider = Self {
            receiver: tokio::sync::Mutex::new(Some(rx)),
        };
        (tx, provider)
    }

    /// Creates a provider from an existing receiver.
    pub fn from_receiver(receiver: tokio::sync::oneshot::Receiver<Option<Password>>) -> Self {
        Self {
            receiver: tokio::sync::Mutex::new(Some(receiver)),
        }
    }
}

#[cfg(feature = "aes")]
impl AsyncPasswordProvider for InteractivePasswordProvider {
    fn get_password(&self) -> Pin<Box<dyn Future<Output = Option<Password>> + Send + '_>> {
        Box::pin(async move {
            let mut guard = self.receiver.lock().await;
            if let Some(rx) = guard.take() {
                rx.await.ok().flatten()
            } else {
                // Already consumed
                None
            }
        })
    }
}

#[cfg(feature = "aes")]
impl std::fmt::Debug for InteractivePasswordProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractivePasswordProvider").finish()
    }
}

/// Callback-based password provider that invokes an async function.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::async_password::CallbackPasswordProvider;
///
/// let provider = CallbackPasswordProvider::new(|| async {
///     // Your async password retrieval logic here
///     Some(Password::new("callback_password"))
/// });
/// ```
#[cfg(feature = "aes")]
pub struct CallbackPasswordProvider<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Option<Password>> + Send,
{
    callback: F,
}

#[cfg(feature = "aes")]
impl<F, Fut> CallbackPasswordProvider<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Option<Password>> + Send,
{
    /// Creates a new callback-based password provider.
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

#[cfg(feature = "aes")]
impl<F, Fut> AsyncPasswordProvider for CallbackPasswordProvider<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Option<Password>> + Send + 'static,
{
    fn get_password(&self) -> Pin<Box<dyn Future<Output = Option<Password>> + Send + '_>> {
        Box::pin((self.callback)())
    }
}

#[cfg(feature = "aes")]
impl<F, Fut> std::fmt::Debug for CallbackPasswordProvider<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Option<Password>> + Send,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackPasswordProvider").finish()
    }
}

#[cfg(all(test, feature = "aes"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_async_password_with_value() {
        let provider = AsyncPassword::new("test_password");
        let password = provider.get_password().await;
        assert!(password.is_some());
        assert_eq!(password.unwrap().as_str(), "test_password");
    }

    #[tokio::test]
    async fn test_async_password_none() {
        let provider = AsyncPassword::none();
        let password = provider.get_password().await;
        assert!(password.is_none());
    }

    #[tokio::test]
    async fn test_interactive_password_provider() {
        let (tx, provider) = InteractivePasswordProvider::new();

        // Send password in a separate task
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(Some(Password::new("interactive_password"))).ok();
        });

        let password = provider.get_password().await;
        assert!(password.is_some());
        assert_eq!(password.unwrap().as_str(), "interactive_password");
    }

    #[tokio::test]
    async fn test_interactive_password_provider_cancelled() {
        let (tx, provider) = InteractivePasswordProvider::new();

        // Drop sender without sending (simulates cancel)
        drop(tx);

        let password = provider.get_password().await;
        assert!(password.is_none());
    }

    #[tokio::test]
    async fn test_callback_password_provider() {
        let provider =
            CallbackPasswordProvider::new(|| async { Some(Password::new("callback_password")) });

        let password = provider.get_password().await;
        assert!(password.is_some());
        assert_eq!(password.unwrap().as_str(), "callback_password");
    }

    #[tokio::test]
    async fn test_async_password_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AsyncPassword>();
    }
}
