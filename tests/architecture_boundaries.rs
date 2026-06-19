use std::{
    fs,
    path::{Path, PathBuf},
};

#[test]
fn cli_and_tui_remain_thin_clients_without_obs_client_imports() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let violations = ["src/cli", "src/tui"]
        .into_iter()
        .flat_map(|dir| rust_files(&root.join(dir)))
        .flat_map(|path| disallowed_obs_client_imports(&root, &path))
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "CLI and TUI modules must not import OBS client implementation modules:\n{}",
        violations.join("\n")
    );
}

#[test]
fn thin_client_guard_catches_grouped_obs_client_imports() {
    let imports = [
        "use crate::obs::client::ObsClient;",
        "use crate::obs::{client::ObsEvent};",
        "use crate::{config::Config, obs::client::ObsClient};",
        "use obsctl_rs::obs::client::ObsEvent;",
        "use obsctl_rs::{ipc::protocol::ServerMessage, obs::{client::ObsClient}};",
        "use obs::client::ObsEvent;",
        "use obs::{client::ObsEvent};",
    ];

    for import in imports {
        let normalized = normalize_import(import);
        assert!(
            imports_obs_client(&normalized),
            "should catch thin-client OBS import: {import}"
        );
    }
}

fn disallowed_obs_client_imports(root: &Path, path: &Path) -> Vec<String> {
    let source = fs::read_to_string(path).unwrap();

    extract_imports(&production_source(&source))
        .into_iter()
        .filter(|import| imports_obs_client(import))
        .map(|import| format!("{} imports `{import}`", relative_path(root, path)))
        .collect()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files(dir, &mut files);
    files.sort();
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn production_source(source: &str) -> String {
    strip_cfg_test_modules(&strip_comments(source))
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();

    while let Some(ch) = chars.next() {
        match (ch, chars.peek().copied()) {
            ('/', Some('/')) => {
                chars.next();
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = '\0';
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                    }
                    if previous == '*' && comment_ch == '/' {
                        break;
                    }
                    previous = comment_ch;
                }
            }
            _ => out.push(ch),
        }
    }

    out
}

fn strip_cfg_test_modules(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    const CFG_TEST: &str = "#[cfg(test)]";

    while let Some(attr_offset) = source[cursor..].find(CFG_TEST) {
        let attr_start = cursor + attr_offset;
        out.push_str(&source[cursor..attr_start]);

        let after_attr = attr_start + CFG_TEST.len();
        let Some(mod_offset) = source[after_attr..].find("mod tests") else {
            out.push_str(&source[attr_start..after_attr]);
            cursor = after_attr;
            continue;
        };
        let mod_start = after_attr + mod_offset;
        if !source[after_attr..mod_start].trim().is_empty() {
            out.push_str(&source[attr_start..after_attr]);
            cursor = after_attr;
            continue;
        }

        let Some(open_offset) = source[mod_start..].find('{') else {
            out.push_str(&source[attr_start..after_attr]);
            cursor = after_attr;
            continue;
        };
        let open_brace = mod_start + open_offset;
        let Some(module_end) = matching_brace_end(source, open_brace) else {
            out.push_str(&source[attr_start..]);
            return out;
        };

        out.push('\n');
        cursor = module_end;
    }

    out.push_str(&source[cursor..]);
    out
}

fn matching_brace_end(source: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = source[open_brace..].char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        match ch {
            '"' => skip_string_literal(&mut chars),
            '\'' => skip_char_literal(&mut chars),
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open_brace + offset + ch.len_utf8());
                }
            }
            _ => {}
        }
    }

    None
}

fn skip_string_literal(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    let mut escaped = false;
    for (_, ch) in chars.by_ref() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            break;
        }
    }
}

fn skip_char_literal(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    let mut escaped = false;
    for (_, ch) in chars.by_ref() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '\'' {
            break;
        }
    }
}

fn extract_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut cursor = 0;

    while let Some(offset) = source[cursor..].find("use") {
        let start = cursor + offset;
        if !is_keyword_at(source, start, "use") {
            cursor = start + "use".len();
            continue;
        }

        let Some(end_offset) = source[start..].find(';') else {
            break;
        };
        let end = start + end_offset + 1;
        imports.push(normalize_import(&source[start..end]));
        cursor = end;
    }

    imports
}

fn normalize_import(statement: &str) -> String {
    statement
        .trim()
        .strip_prefix("use")
        .unwrap_or(statement)
        .trim_end_matches(';')
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn imports_obs_client(import: &str) -> bool {
    imports_crate_nested_module(import, "obs", "client")
        || imports_package_nested_module(import, "obsctl_rs", "obs", "client")
        || imports_bare_nested_module(import, "obs", "client")
}

fn imports_crate_nested_module(import: &str, module: &str, nested: &str) -> bool {
    import == format!("crate::{module}::{nested}")
        || import.starts_with(&format!("crate::{module}::{nested}::"))
        || import.starts_with(&format!("crate::{module}::{{{nested}::"))
        || import.starts_with(&format!("crate::{{{module}::{nested}::"))
        || import.starts_with(&format!("crate::{{{module}::{{{nested}::"))
        || import.contains(&format!(",{module}::{nested}::"))
        || import.contains(&format!(",{module}::{{{nested}::"))
}

fn imports_package_nested_module(import: &str, package: &str, module: &str, nested: &str) -> bool {
    import == format!("{package}::{module}::{nested}")
        || import.starts_with(&format!("{package}::{module}::{nested}::"))
        || import.starts_with(&format!("{package}::{module}::{{{nested}::"))
        || import.starts_with(&format!("{package}::{{{module}::{nested}::"))
        || import.starts_with(&format!("{package}::{{{module}::{{{nested}::"))
        || import.contains(&format!(",{module}::{nested}::"))
        || import.contains(&format!(",{module}::{{{nested}::"))
}

fn imports_bare_nested_module(import: &str, module: &str, nested: &str) -> bool {
    import == format!("{module}::{nested}")
        || import.starts_with(&format!("{module}::{nested}::"))
        || import.starts_with(&format!("{module}::{{{nested}::"))
}

fn is_keyword_at(source: &str, start: usize, keyword: &str) -> bool {
    let before = source[..start].chars().next_back();
    let after = source[start + keyword.len()..].chars().next();

    !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
}

fn is_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
