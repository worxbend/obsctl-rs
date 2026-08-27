//! Locale resolution and runtime translation loading.
//!
//! English strings are embedded into the binary at compile time from
//! `locales/en.yml` (see the `rust_i18n::i18n!` invocation in `lib.rs`).
//! Other locales — currently only Ukrainian — are loaded at runtime from
//! `<config_dir>/locales/<locale>.yml`. If that override file is missing or
//! fails to parse, lookups silently fall back to the embedded English
//! strings: a bad or absent locale file must never stop the CLI from
//! running. Use the `t!` macro (re-exported by `rust_i18n`) to look up
//! translations, e.g. `rust_i18n::t!("cli.init.success", path = path)`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rust_i18n::Backend;

/// Locales supported by obsctl. Keep in sync with `locales/en.yml` and
/// `contrib/locales/uk.yml`.
pub const SUPPORTED_LOCALES: &[&str] = &["en", "uk"];

const DEFAULT_LOCALE: &str = "en";

static LOCALES_DIR_OVERRIDE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Backend that loads locale override files from `<config_dir>/locales/*.yml`
/// on first use. Constructed lazily by the `rust_i18n::i18n!` macro, so any
/// directory resolved by [`init`] must be recorded before the first
/// translation lookup happens.
pub struct FileBackend {
    messages: HashMap<String, HashMap<String, String>>,
}

impl FileBackend {
    pub fn new() -> Self {
        let dir = LOCALES_DIR_OVERRIDE
            .get()
            .cloned()
            .unwrap_or(None)
            .or_else(|| crate::config::paths::locales_dir(None));
        Self::load(dir.as_deref())
    }

    fn load(dir: Option<&Path>) -> Self {
        let mut messages = HashMap::new();
        let Some(dir) = dir else {
            return Self { messages };
        };

        for locale in SUPPORTED_LOCALES {
            let path = dir.join(format!("{locale}.yml"));
            if !path.is_file() {
                continue;
            }
            if crate::support::fs::has_symlink(&path) {
                tracing::warn!(
                    path = %path.display(),
                    "ignoring symlinked locale override file"
                );
                continue;
            }

            let content = match crate::support::fs::read_file_no_follow(&path) {
                Ok(content) => content,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "failed to read locale override file"
                    );
                    continue;
                }
            };

            let value: serde_yaml::Value = match serde_yaml::from_str(&content) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "failed to parse locale override file"
                    );
                    continue;
                }
            };

            let mut flat = HashMap::new();
            flatten_into(&value, String::new(), &mut flat);
            messages.insert((*locale).to_string(), flat);
        }

        Self { messages }
    }
}

impl Default for FileBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for FileBackend {
    fn available_locales(&self) -> Vec<Cow<'_, str>> {
        self.messages
            .keys()
            .map(|k| Cow::Borrowed(k.as_str()))
            .collect()
    }

    fn translate(&self, locale: &str, key: &str) -> Option<Cow<'_, str>> {
        self.messages
            .get(locale)?
            .get(key)
            .map(|v| Cow::Borrowed(v.as_str()))
    }

    fn messages_for_locale(&self, locale: &str) -> Option<Vec<(Cow<'_, str>, Cow<'_, str>)>> {
        self.messages.get(locale).map(|m| {
            m.iter()
                .map(|(k, v)| (Cow::Borrowed(k.as_str()), Cow::Borrowed(v.as_str())))
                .collect()
        })
    }
}

/// Flatten a nested YAML mapping into dot-joined keys, mirroring the format
/// rust-i18n uses for its own compile-time embedded locale files (so a
/// locale override file can simply reuse `locales/en.yml`'s structure).
fn flatten_into(value: &serde_yaml::Value, prefix: String, out: &mut HashMap<String, String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let Some(key) = k.as_str() else { continue };
                let next_prefix = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_into(v, next_prefix, out);
            }
        }
        serde_yaml::Value::String(s) => {
            out.insert(prefix, s.clone());
        }
        serde_yaml::Value::Null => {
            out.insert(prefix, String::new());
        }
        serde_yaml::Value::Bool(b) => {
            out.insert(prefix, b.to_string());
        }
        serde_yaml::Value::Number(n) => {
            out.insert(prefix, n.to_string());
        }
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Tagged(_) => {
            // Locale values are scalars; arrays/tagged values aren't supported.
        }
    }
}

/// Resolve the effective locale from (in priority order): the `OBSCTL_LOCALE`
/// env var, the `ui.locale` config field, then the default (`en`). Falls
/// back to `en` if the resolved value isn't in [`SUPPORTED_LOCALES`].
pub fn resolve_locale(env_override: Option<String>, config_locale: Option<&str>) -> String {
    let candidate = env_override
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or(config_locale)
        .unwrap_or(DEFAULT_LOCALE)
        .to_ascii_lowercase();

    if SUPPORTED_LOCALES.contains(&candidate.as_str()) {
        candidate
    } else {
        if candidate != DEFAULT_LOCALE {
            tracing::warn!(locale = %candidate, "unsupported locale, falling back to en");
        }
        DEFAULT_LOCALE.to_string()
    }
}

/// Resolve and set the active locale for the process. `config_path` is the
/// config file that would be (or was) loaded, used to find the sibling
/// `locales/` override directory; `config_locale` is the already-loaded
/// `ui.locale` field, if any. Must run before the first `t!()` call to take
/// effect, since the override directory is only read once.
pub fn init(config_path: Option<&Path>, config_locale: Option<&str>) {
    let _ = LOCALES_DIR_OVERRIDE.set(crate::config::paths::locales_dir(config_path));
    let env_override = crate::support::validation::read_env_token("OBSCTL_LOCALE");
    let locale = resolve_locale(env_override, config_locale);
    rust_i18n::set_locale(&locale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_locale_defaults_to_en() {
        assert_eq!(resolve_locale(None, None), "en");
    }

    #[test]
    fn resolve_locale_uses_config_value() {
        assert_eq!(resolve_locale(None, Some("uk")), "uk");
    }

    #[test]
    fn resolve_locale_env_overrides_config() {
        assert_eq!(resolve_locale(Some("uk".to_string()), Some("en")), "uk");
    }

    #[test]
    fn resolve_locale_rejects_unsupported_value() {
        assert_eq!(resolve_locale(None, Some("fr")), "en");
        assert_eq!(resolve_locale(Some("fr".to_string()), None), "en");
    }

    #[test]
    fn resolve_locale_ignores_blank_env_override() {
        assert_eq!(resolve_locale(Some("   ".to_string()), Some("uk")), "uk");
    }

    #[test]
    fn resolve_locale_is_case_insensitive() {
        assert_eq!(resolve_locale(None, Some("UK")), "uk");
        assert_eq!(resolve_locale(Some("Uk".to_string()), None), "uk");
    }

    #[test]
    fn file_backend_loads_flattened_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("uk.yml"),
            "cli:\n  init:\n    success: \"Готово: %{path}\"\n",
        )
        .unwrap();

        let backend = FileBackend::load(Some(dir.path()));
        assert_eq!(
            backend.translate("uk", "cli.init.success"),
            Some(Cow::Borrowed("Готово: %{path}"))
        );
        assert_eq!(backend.translate("en", "cli.init.success"), None);
    }

    #[test]
    fn file_backend_ignores_missing_directory() {
        let backend = FileBackend::load(Some(Path::new("/nonexistent/obsctl-locales-test")));
        assert!(backend.messages.is_empty());
    }

    #[test]
    fn file_backend_ignores_malformed_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("uk.yml"), "not: [valid: yaml").unwrap();

        let backend = FileBackend::load(Some(dir.path()));
        assert!(backend.messages.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_ignores_symlinked_override() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.yml");
        std::fs::write(&real, "greeting: \"hi\"\n").unwrap();
        symlink(&real, dir.path().join("uk.yml")).unwrap();

        let backend = FileBackend::load(Some(dir.path()));
        assert!(backend.messages.is_empty());
    }

    /// Load one of the repository's own locale files the way [`FileBackend`]
    /// loads an override: dot-joined keys, one entry per string.
    fn flatten_locale_file(path: &Path) -> HashMap<String, String> {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
        let value: serde_yaml::Value = serde_yaml::from_str(&text)
            .unwrap_or_else(|error| panic!("{} must be valid YAML: {error}", path.display()));
        let mut flat = HashMap::new();
        flatten_into(&value, String::new(), &mut flat);
        flat
    }

    /// The `%{...}` names a message interpolates, in a comparable order.
    fn placeholders(message: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut rest = message;
        while let Some(start) = rest.find("%{") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            names.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        names.sort();
        names.dedup();
        names
    }

    /// `contrib/locales/uk.yml` is the shipped runtime override, and a key it
    /// is missing does not fail anywhere a developer would see it: the lookup
    /// silently falls back to the embedded English string, so a Ukrainian user
    /// gets a screen in two languages. Diffing the key sets is the only way to
    /// catch that, and doing it by eye every time a key is added does not
    /// survive contact with a busy afternoon.
    #[test]
    fn the_shipped_ukrainian_file_translates_every_english_key() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let en = flatten_locale_file(&root.join("locales/en.yml"));
        let uk = flatten_locale_file(&root.join("contrib/locales/uk.yml"));

        let mut missing: Vec<&str> = en
            .keys()
            .filter(|key| !uk.contains_key(*key))
            .map(String::as_str)
            .collect();
        missing.sort_unstable();
        assert!(
            missing.is_empty(),
            "contrib/locales/uk.yml is missing keys that locales/en.yml defines, \
             so a Ukrainian user sees English for them: {missing:?}"
        );

        let mut stale: Vec<&str> = uk
            .keys()
            .filter(|key| !en.contains_key(*key))
            .map(String::as_str)
            .collect();
        stale.sort_unstable();
        assert!(
            stale.is_empty(),
            "contrib/locales/uk.yml translates keys locales/en.yml no longer has, \
             so nothing will ever look them up: {stale:?}"
        );

        // A translation that drops or misspells a `%{name}` prints the raw
        // placeholder to the user, which is worse than the English sentence it
        // was meant to replace.
        for (key, english) in &en {
            assert_eq!(
                placeholders(english),
                placeholders(&uk[key]),
                "`{key}` interpolates different names in the two locale files"
            );
        }
    }
}
