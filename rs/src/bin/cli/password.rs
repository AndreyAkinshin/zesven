//! Password handling for CLI operations.

use rpassword::prompt_password;
use zesven::Password;

/// Gets a password from the provided option or prompts the user
pub fn get_password(provided: Option<String>, archive_encrypted: bool) -> Option<Password> {
    // If password provided on command line, use it
    if let Some(pwd) = provided {
        return Some(Password::new(pwd));
    }

    // If archive isn't encrypted, no password needed
    if !archive_encrypted {
        return None;
    }

    // Prompt user for password
    match prompt_password("Enter password: ") {
        Ok(pwd) if !pwd.is_empty() => Some(Password::new(pwd)),
        _ => None,
    }
}

/// Prompts for password confirmation (for creating encrypted archives)
pub fn confirm_password() -> Option<Password> {
    let pwd1 = match prompt_password("Enter password: ") {
        Ok(pwd) => pwd,
        Err(_) => return None,
    };

    if pwd1.is_empty() {
        eprintln!("Password cannot be empty");
        return None;
    }

    let pwd2 = match prompt_password("Confirm password: ") {
        Ok(pwd) => pwd,
        Err(_) => return None,
    };

    if pwd1 == pwd2 {
        Some(Password::new(pwd1))
    } else {
        eprintln!("Passwords do not match");
        None
    }
}

/// Prompts for password with optional confirmation
pub fn get_or_prompt_password(provided: Option<String>, confirm: bool) -> Option<Password> {
    if let Some(pwd) = provided {
        return Some(Password::new(pwd));
    }

    if confirm {
        confirm_password()
    } else {
        match prompt_password("Enter password: ") {
            Ok(pwd) if !pwd.is_empty() => Some(Password::new(pwd)),
            _ => None,
        }
    }
}
