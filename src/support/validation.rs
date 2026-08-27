use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Blank,
    ControlCharacters,
    InvalidEnvVarFormat,
    InvalidUtf8,
    ContainsWhitespace,
    MaxLengthExceeded(usize),
}

pub const MAX_TARGET_TOKEN_LENGTH: usize = 256;
pub const MAX_PATH_TOKEN_LENGTH: usize = 1024;
pub const MAX_PASSWORD_LENGTH: usize = 1024;

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Setting an environment variable inside a test, done once.
///
/// Environment variables belong to the whole process, and Rust's test harness
/// runs tests on many threads at once, so one test changing `PATH` or
/// `OBSCTL_CONFIG` is visible to every other test running at that moment.
/// `std::env::set_var` is `unsafe` in edition 2024 for exactly that reason.
/// The two helpers below take the process-wide lock, swap the variable, run the
/// closure, and put the previous value back — including when the variable was
/// absent, which has to be a *removal* rather than setting an empty string.
///
/// This used to be copy-pasted into eight test modules. Getting the restore
/// step subtly wrong in one copy would have leaked state into unrelated tests,
/// with the symptom appearing somewhere else entirely, so it lives in one place.
#[cfg(test)]
pub(crate) mod test_env {
    use std::ffi::OsString;

    /// Run `f` with `name` set to `value` (or removed when `value` is `None`).
    pub(crate) fn with_env_var<R>(name: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
        with_env_var_os(name, value.map(OsString::from), f)
    }

    /// Same as [`with_env_var`], for values that are not valid UTF-8 — the
    /// tests that check how invalid environment content is rejected need to set
    /// raw bytes that a `&str` cannot hold.
    pub(crate) fn with_env_var_os<R>(
        name: &str,
        value: Option<OsString>,
        f: impl FnOnce() -> R,
    ) -> R {
        // A poisoned lock means some other test panicked while holding it. The
        // data it guards is the environment itself, not a Rust value that could
        // be left half-updated, so carrying on is correct here: refusing would
        // turn one failing test into a cascade of unrelated failures.
        let _lock = super::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os(name);
        set_or_remove(name, value);

        let result = f();

        set_or_remove(name, previous);
        result
    }

    fn set_or_remove(name: &str, value: Option<OsString>) {
        // SAFETY: the caller holds `test_env_lock`, so no other helper in this
        // module is touching the environment concurrently. Tests that read the
        // environment without going through these helpers are the remaining
        // hazard, and there are none in this crate.
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordConfigError {
    ConflictingPasswordSources,
    PlaintextPasswordHasControlCharacters,
    PlaintextPasswordTooLong,
    PasswordEnvError(ValidationError),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "must not be blank"),
            Self::ControlCharacters => {
                write!(f, "must not contain control characters")
            }
            Self::InvalidEnvVarFormat => {
                write!(f, "must be a valid environment variable name")
            }
            Self::InvalidUtf8 => {
                write!(f, "must be valid UTF-8")
            }
            Self::ContainsWhitespace => {
                write!(f, "must not contain whitespace")
            }
            Self::MaxLengthExceeded(max_length) => {
                write!(f, "must be at most {max_length} characters")
            }
        }
    }
}

impl fmt::Display for PasswordConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingPasswordSources => {
                write!(f, "password and password_env are both set")
            }
            Self::PlaintextPasswordHasControlCharacters => {
                write!(f, "password contains control characters")
            }
            Self::PlaintextPasswordTooLong => {
                write!(f, "password exceeds maximum length")
            }
            Self::PasswordEnvError(error) => {
                write!(f, "{error}")
            }
        }
    }
}

pub fn password_config_error_message(error: &PasswordConfigError) -> &'static str {
    match error {
        PasswordConfigError::ConflictingPasswordSources => {
            "connection.password and connection.password_env are mutually exclusive"
        }
        PasswordConfigError::PlaintextPasswordHasControlCharacters => {
            "connection.password must not contain control characters"
        }
        PasswordConfigError::PlaintextPasswordTooLong => {
            "connection.password exceeds maximum length"
        }
        // Every `ValidationError` variant is spelled out rather than covered by
        // a `_` arm. An earlier catch-all reported any unlisted variant as a
        // control-character problem, so a variant added later would have been
        // described to the user as something it is not, with nothing failing to
        // say so. Listing them all makes the compiler point at this match the
        // next time the enum grows.
        PasswordConfigError::PasswordEnvError(error) => match error {
            ValidationError::InvalidUtf8 => "connection.password_env must be valid UTF-8",
            ValidationError::InvalidEnvVarFormat | ValidationError::Blank => {
                "connection.password_env must be a valid environment variable name"
            }
            ValidationError::MaxLengthExceeded(_) => {
                "connection.password_env value exceeds maximum length"
            }
            ValidationError::ControlCharacters | ValidationError::ContainsWhitespace => {
                "connection.password_env value must not contain control characters"
            }
        },
    }
}

pub fn trim_and_validate_token_with_max_len(
    value: &str,
    max_length: usize,
) -> Result<String, ValidationError> {
    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError::ControlCharacters);
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::Blank);
    }
    if trimmed.chars().count() > max_length {
        return Err(ValidationError::MaxLengthExceeded(max_length));
    }
    Ok(trimmed.to_string())
}

/// Read an environment variable as a safe control-free token.
///
/// - Missing variable -> `None`
/// - Non-UTF-8 value -> `None`
/// - Blank / control characters -> `None`
/// - Otherwise -> trimmed, validated token
pub fn read_env_token(name: &str) -> Option<String> {
    let raw = std::env::var_os(name)?;
    let value = raw.to_str()?;
    trim_and_validate_token_with_max_len(value, MAX_PATH_TOKEN_LENGTH).ok()
}

/// Read an environment variable intended to be used as a path token.
///
/// Applies the same validation as [`read_env_token`], and additionally rejects
/// values containing `.` or `..` path components, so the result cannot escape
/// the directory it is joined onto.
pub fn read_env_path_token(name: &str) -> Option<String> {
    let value = read_env_token(name)?;
    if crate::support::fs::has_path_traversal(Path::new(&value)) {
        None
    } else {
        Some(value)
    }
}

/// Read an environment variable intended to contain a password value.
///
/// - Invalid variable name -> `Err(ValidationError::InvalidEnvVarFormat | Blank | ControlCharacters)`
/// - Non-UTF8 value -> `Err(ValidationError::InvalidUtf8)`
/// - Overly long value -> `Err(ValidationError::MaxLengthExceeded(MAX_PASSWORD_LENGTH))`
/// - Control characters in value -> `Err(ValidationError::ControlCharacters)`
/// - Missing variable -> `Ok(None)`
/// - Present and valid value -> `Ok(Some(value))`
pub fn read_password_env_value(name: &str) -> Result<Option<String>, ValidationError> {
    validate_env_var_name(name)?;
    let Some(value) = read_env_value(name)? else {
        return Ok(None);
    };
    if value.chars().count() > MAX_PASSWORD_LENGTH {
        return Err(ValidationError::MaxLengthExceeded(MAX_PASSWORD_LENGTH));
    }
    validate_no_control_characters(&value)?;
    Ok(Some(value))
}

/// Read an environment variable as UTF-8 text.
///
/// - Missing variable -> `Ok(None)`
/// - Non-UTF8 value -> `Err(ValidationError::InvalidUtf8)`
/// - UTF-8 value -> `Ok(Some(value))`
fn read_env_value(name: &str) -> Result<Option<String>, ValidationError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ValidationError::InvalidUtf8),
    }
}

pub fn validate_env_var_name(value: &str) -> Result<String, ValidationError> {
    let token = trim_and_validate_token_with_max_len(value, MAX_PATH_TOKEN_LENGTH)?;
    let mut chars = token.chars();
    let first = chars.next().ok_or(ValidationError::Blank)?;

    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(ValidationError::InvalidEnvVarFormat);
    }

    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(ValidationError::InvalidEnvVarFormat);
    }

    Ok(token)
}

pub fn trim_and_validate_path_token(value: &str) -> Result<String, ValidationError> {
    trim_and_validate_token_with_max_len(value, MAX_PATH_TOKEN_LENGTH)
}

pub fn validate_no_control_characters(value: &str) -> Result<(), ValidationError> {
    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError::ControlCharacters);
    }
    Ok(())
}

pub fn validate_no_control_or_whitespace(value: &str) -> Result<(), ValidationError> {
    validate_no_control_characters(value)?;
    if value.chars().any(|c| c.is_whitespace()) {
        return Err(ValidationError::ContainsWhitespace);
    }
    Ok(())
}

pub fn resolve_connection_password(
    password: Option<&str>,
    password_env: &str,
) -> Result<Option<String>, PasswordConfigError> {
    if password.is_some() && !password_env.is_empty() {
        return Err(PasswordConfigError::ConflictingPasswordSources);
    }

    if let Some(password) = password {
        if password.chars().count() > MAX_PASSWORD_LENGTH {
            return Err(PasswordConfigError::PlaintextPasswordTooLong);
        }

        validate_no_control_characters(password)
            .map_err(|_| PasswordConfigError::PlaintextPasswordHasControlCharacters)?;
        return Ok(Some(password.to_string()));
    }

    if password_env.is_empty() {
        return Ok(None);
    }

    read_password_env_value(password_env).map_err(PasswordConfigError::PasswordEnvError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::validation::test_env::{with_env_var, with_env_var_os};

    #[test]
    fn validates_control_characters() {
        assert_eq!(validate_no_control_characters("ok"), Ok(()));
        assert!(validate_no_control_characters("bad\n").is_err());
    }

    #[test]
    fn validates_token_with_max_len() {
        let max_len = MAX_TARGET_TOKEN_LENGTH;
        assert!(trim_and_validate_token_with_max_len("abc", max_len).is_ok());
        assert_eq!(
            trim_and_validate_token_with_max_len(&"a".repeat(max_len + 1), max_len),
            Err(ValidationError::MaxLengthExceeded(max_len))
        );
        assert_eq!(
            trim_and_validate_token_with_max_len(&"a".repeat(max_len), max_len).unwrap(),
            "a".repeat(max_len)
        );
    }

    #[test]
    fn validates_whitespace() {
        assert_eq!(validate_no_control_or_whitespace("topic"), Ok(()));
        assert!(validate_no_control_or_whitespace("bad topic").is_err());
    }

    #[test]
    fn read_env_token_returns_trimmed_safe_value() {
        with_env_var("OBSCTL_TEST_ENV_TOKEN", Some("  foo  "), || {
            assert_eq!(
                read_env_token("OBSCTL_TEST_ENV_TOKEN").as_deref(),
                Some("foo")
            );
        });
    }

    #[test]
    fn read_env_token_rejects_excessive_length() {
        let token = "a".repeat(MAX_PATH_TOKEN_LENGTH + 1);
        with_env_var("OBSCTL_TEST_ENV_TOKEN", Some(&token), || {
            assert_eq!(read_env_token("OBSCTL_TEST_ENV_TOKEN"), None);
        });
    }

    #[test]
    fn read_env_token_rejects_control_characters() {
        with_env_var("OBSCTL_TEST_ENV_TOKEN", Some("abc\n123"), || {
            assert_eq!(read_env_token("OBSCTL_TEST_ENV_TOKEN"), None);
        });
    }

    #[test]
    fn read_env_path_token_rejects_traversal() {
        with_env_var("OBSCTL_TEST_ENV_TOKEN", Some("/tmp/../obsctl"), || {
            assert_eq!(read_env_path_token("OBSCTL_TEST_ENV_TOKEN"), None);
        });
    }

    #[test]
    fn read_env_path_token_allows_safe_path() {
        with_env_var("OBSCTL_TEST_ENV_TOKEN", Some("/tmp/obsctl"), || {
            assert_eq!(
                read_env_path_token("OBSCTL_TEST_ENV_TOKEN"),
                Some("/tmp/obsctl".to_string())
            );
        });
    }

    #[test]
    fn validate_env_var_name_rejects_excessive_length() {
        let name = "a".repeat(MAX_PATH_TOKEN_LENGTH + 1);
        assert_eq!(
            validate_env_var_name(&name),
            Err(ValidationError::MaxLengthExceeded(MAX_PATH_TOKEN_LENGTH))
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_env_token_rejects_non_unicode_value() {
        use std::os::unix::ffi::OsStringExt;

        let value = {
            let mut bytes = b"abc".to_vec();
            bytes.push(0xff);
            std::ffi::OsString::from_vec(bytes)
        };
        with_env_var_os("OBSCTL_TEST_ENV_TOKEN", Some(value), || {
            assert_eq!(read_env_token("OBSCTL_TEST_ENV_TOKEN"), None);
        });
    }

    #[test]
    fn read_env_value_returns_none_when_missing() {
        with_env_var("OBSCTL_TEST_ENV_VALUE_MISSING", None, || {
            assert!(
                read_env_value("OBSCTL_TEST_ENV_VALUE_MISSING")
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn read_env_value_returns_value() {
        with_env_var("OBSCTL_TEST_ENV_VALUE", Some("abc"), || {
            assert_eq!(
                read_env_value("OBSCTL_TEST_ENV_VALUE").unwrap(),
                Some("abc".to_string())
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn read_env_value_rejects_non_unicode() {
        use std::os::unix::ffi::OsStringExt;

        let value = {
            let mut bytes = b"abc".to_vec();
            bytes.push(0xff);
            std::ffi::OsString::from_vec(bytes)
        };
        with_env_var_os("OBSCTL_TEST_ENV_VALUE", Some(value), || {
            assert_eq!(
                read_env_value("OBSCTL_TEST_ENV_VALUE"),
                Err(ValidationError::InvalidUtf8)
            );
        });
    }

    #[test]
    fn read_password_env_value_rejects_invalid_name() {
        assert_eq!(
            read_password_env_value("1BAD_NAME"),
            Err(ValidationError::InvalidEnvVarFormat)
        );
    }

    #[test]
    fn read_password_env_value_rejects_non_unicode_value() {
        with_env_var_os(
            "OBSCTL_TEST_PASSWORD_ENV_INVALID_UTF8",
            {
                use std::os::unix::ffi::OsStringExt;

                let mut bytes = b"abc".to_vec();
                bytes.push(0xff);
                Some(std::ffi::OsString::from_vec(bytes))
            },
            || {
                assert_eq!(
                    read_password_env_value("OBSCTL_TEST_PASSWORD_ENV_INVALID_UTF8"),
                    Err(ValidationError::InvalidUtf8)
                );
            },
        );
    }

    #[test]
    fn read_password_env_value_rejects_control_characters() {
        with_env_var("OBSCTL_TEST_PASSWORD_ENV_CTRL", Some("abc\n123"), || {
            assert_eq!(
                read_password_env_value("OBSCTL_TEST_PASSWORD_ENV_CTRL"),
                Err(ValidationError::ControlCharacters)
            );
        });
    }

    #[test]
    fn read_password_env_value_rejects_excessive_length() {
        let value = "a".repeat(MAX_PASSWORD_LENGTH + 1);
        with_env_var("OBSCTL_TEST_PASSWORD_ENV_LONG", Some(&value), || {
            assert_eq!(
                read_password_env_value("OBSCTL_TEST_PASSWORD_ENV_LONG"),
                Err(ValidationError::MaxLengthExceeded(MAX_PASSWORD_LENGTH))
            );
        });
    }

    #[test]
    fn read_password_env_value_returns_none_when_missing() {
        with_env_var("OBSCTL_TEST_PASSWORD_ENV_NONE", None, || {
            assert!(
                read_password_env_value("OBSCTL_TEST_PASSWORD_ENV_NONE")
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn read_password_env_value_accepts_value() {
        with_env_var("OBSCTL_TEST_PASSWORD_ENV_OK", Some("secret"), || {
            assert_eq!(
                read_password_env_value("OBSCTL_TEST_PASSWORD_ENV_OK").unwrap(),
                Some("secret".to_string())
            );
        });
    }

    #[test]
    fn password_config_error_message_for_conflicting_sources() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::ConflictingPasswordSources),
            "connection.password and connection.password_env are mutually exclusive"
        );
    }

    #[test]
    fn password_config_error_message_for_plaintext_control_chars() {
        assert_eq!(
            password_config_error_message(
                &PasswordConfigError::PlaintextPasswordHasControlCharacters
            ),
            "connection.password must not contain control characters"
        );
    }

    #[test]
    fn password_config_error_message_for_password_env_invalid_utf8() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PasswordEnvError(
                ValidationError::InvalidUtf8
            )),
            "connection.password_env must be valid UTF-8"
        );
    }

    #[test]
    fn password_config_error_message_for_password_env_invalid_name() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PasswordEnvError(
                ValidationError::InvalidEnvVarFormat
            )),
            "connection.password_env must be a valid environment variable name"
        );
    }

    #[test]
    fn password_config_error_message_for_password_env_blank() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PasswordEnvError(
                ValidationError::Blank
            )),
            "connection.password_env must be a valid environment variable name"
        );
    }

    #[test]
    fn password_config_error_message_for_password_env_control_characters() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PasswordEnvError(
                ValidationError::ControlCharacters
            )),
            "connection.password_env value must not contain control characters"
        );
    }

    #[test]
    fn password_config_error_message_for_password_env_too_long() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PasswordEnvError(
                ValidationError::MaxLengthExceeded(MAX_PASSWORD_LENGTH)
            )),
            "connection.password_env value exceeds maximum length"
        );
    }

    #[test]
    fn password_config_error_message_for_plaintext_too_long() {
        assert_eq!(
            password_config_error_message(&PasswordConfigError::PlaintextPasswordTooLong),
            "connection.password exceeds maximum length"
        );
    }

    /// Every `ValidationError` variant, in declaration order.
    ///
    /// `password_config_error_message` renders each variant into its own
    /// sentence. Adding a variant without extending that match used to be
    /// invisible, because the match ended in a catch-all. This list plus the
    /// count assertion below is the same guard `domain::errors` and
    /// `ipc::protocol` use: the compiler rejects a missing arm, and the count
    /// rejects a variant added without a case here.
    const ALL_VALIDATION_ERRORS: &[ValidationError] = &[
        ValidationError::Blank,
        ValidationError::ControlCharacters,
        ValidationError::InvalidEnvVarFormat,
        ValidationError::InvalidUtf8,
        ValidationError::ContainsWhitespace,
        ValidationError::MaxLengthExceeded(MAX_PASSWORD_LENGTH),
    ];

    #[test]
    fn every_validation_error_has_a_password_env_message() {
        const VALIDATION_ERROR_VARIANT_COUNT: usize = 6;

        assert_eq!(
            ALL_VALIDATION_ERRORS.len(),
            VALIDATION_ERROR_VARIANT_COUNT,
            "a ValidationError variant was added without a case in ALL_VALIDATION_ERRORS"
        );

        for error in ALL_VALIDATION_ERRORS {
            let message = password_config_error_message(&PasswordConfigError::PasswordEnvError(
                error.clone(),
            ));
            assert!(
                message.starts_with("connection.password_env"),
                "{error:?} must be reported as a connection.password_env problem"
            );
            // The Display text and this table are two separate wordings of the
            // same failure; both must actually say something.
            assert!(!error.to_string().is_empty(), "{error:?} has no Display");
        }
    }

    #[test]
    fn resolve_connection_password_rejects_oversized_plaintext() {
        let long_password = "a".repeat(MAX_PASSWORD_LENGTH + 1);
        assert!(resolve_connection_password(Some(&long_password), "").is_err());
        assert!(matches!(
            resolve_connection_password(Some(&long_password), ""),
            Err(PasswordConfigError::PlaintextPasswordTooLong)
        ));
    }
}
