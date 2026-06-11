//! Auto-loading of `.env` / `.env.local` from the current directory.
//!
//! Credential resolution for the manifest flow follows the precedence
//!
//! ```text
//! shell env  >  .env  >  .env.local  >  config file
//! ```
//!
//! The first three layers are realized here by loading the dotenv files *into
//! the process environment*, but only for keys that aren't already set. Loading
//! `.env` before `.env.local` (and never overwriting an existing value) yields
//! exactly that order: a real shell variable always wins, then `.env`, then
//! `.env.local`. The `> config file` tail is applied later, in
//! `crate::credentials`, by treating the (now dotenv-populated) environment as
//! the high-priority layer and stored config as the fallback.
//!
//! Loading is intentionally scoped to the commands that consume credentials
//! (`manifest sync`, `auth status`) rather than every invocation: the
//! profile-based flow keeps its token in its own config and never reads these
//! env vars, and a global load would pull a developer's real `.env` into
//! unrelated subprocesses.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

/// The two files we look for, in priority order (earlier wins).
const DOTENV_FILES: &[(&str, &str)] = &[
    (".env", "the .env file"),
    (".env.local", "the .env.local file"),
];

/// Load `.env` then `.env.local` from the current directory into the process
/// environment, setting only keys that aren't already present (so the real
/// shell environment and earlier files win). Returns, for each key this call
/// actually set, the human-readable name of the file it came from — used by
/// `auth status` to explain provenance.
pub fn load_dotenv_cwd() -> BTreeMap<String, &'static str> {
    let plan = plan_dotenv_load(Path::new("."), |k| env::var_os(k).is_some());
    let mut origins = BTreeMap::new();
    for (key, value, origin) in plan {
        // Edition 2021: `set_var` is safe. We only ever set keys we proved are
        // currently unset, so this never clobbers a shell-provided value.
        env::set_var(&key, &value);
        origins.insert(key, origin);
    }
    origins
}

/// Pure planner: decide which keys to set and from which file, given a way to
/// ask whether a key is already present in the *external* environment. Keeps
/// the precedence logic testable without mutating global state.
///
/// `shell_has(key)` must answer for the real environment only; cross-file
/// precedence (`.env` beats `.env.local`) is tracked internally.
fn plan_dotenv_load(
    dir: &Path,
    shell_has: impl Fn(&str) -> bool,
) -> Vec<(String, String, &'static str)> {
    // key -> (value, origin); first file to claim a key keeps it.
    let mut planned: BTreeMap<String, (String, &'static str)> = BTreeMap::new();
    for (file, origin) in DOTENV_FILES {
        let Ok(content) = fs::read_to_string(dir.join(file)) else {
            continue;
        };
        // Within a single file, the last assignment of a key wins (dotenv
        // convention); fold before applying cross-layer precedence.
        let mut per_file: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in parse_dotenv(&content) {
            per_file.insert(k, v);
        }
        for (k, v) in per_file {
            if shell_has(&k) || planned.contains_key(&k) {
                continue;
            }
            planned.insert(k, (v, origin));
        }
    }
    planned
        .into_iter()
        .map(|(k, (v, origin))| (k, v, origin))
        .collect()
}

/// Parse dotenv text into `(key, value)` pairs in file order. Lenient: blank
/// lines, comments, and anything that doesn't look like an assignment are
/// skipped rather than erroring, because these files are hand-authored.
pub fn parse_dotenv(content: &str) -> Vec<(String, String)> {
    content.lines().filter_map(parse_dotenv_line).collect()
}

/// Parse one line into `(key, value)`, or `None` for blanks/comments/garbage.
/// Understands an optional `export ` prefix, whitespace around `=`, and
/// double-quoted (with the same escapes `format_env_line` emits), single-quoted
/// (literal), or bare values.
fn parse_dotenv_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed
        .strip_prefix("export ")
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let eq = rest.find('=')?;
    let key = rest[..eq].trim_end();
    if !is_valid_key(key) {
        return None;
    }
    let raw_value = rest[eq + 1..].trim();
    Some((key.to_string(), unquote(raw_value)))
}

/// A POSIX-ish env var name: starts with a letter or `_`, then alphanumerics or
/// `_`. Mirrors `destinations::parse_env_key` so reads and writes agree.
fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Unquote a dotenv value. Double quotes honor the escapes our writer emits
/// (`\n \r \t \\ \" \$ \``); single quotes are literal; bare values are taken
/// verbatim (already trimmed by the caller).
fn unquote(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if raw.len() >= 2 && bytes[0] == b'"' && bytes[raw.len() - 1] == b'"' {
        unescape_double(&raw[1..raw.len() - 1])
    } else if raw.len() >= 2 && bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\'' {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}

fn unescape_double(inner: &str) -> String {
    // Most values carry no escape at all; skip the char-by-char rebuild then.
    if !inner.contains('\\') {
        return inner.to_string();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('$') => out.push('$'),
            Some('`') => out.push('`'),
            // Unknown escape: keep the backslash and the char literally.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[test]
    fn parses_common_line_shapes() {
        assert_eq!(
            parse_dotenv_line("FOO=bar"),
            Some(("FOO".into(), "bar".into()))
        );
        assert_eq!(
            parse_dotenv_line("export FOO=bar"),
            Some(("FOO".into(), "bar".into()))
        );
        assert_eq!(
            parse_dotenv_line("  FOO = bar "),
            Some(("FOO".into(), "bar".into()))
        );
        assert_eq!(parse_dotenv_line("# comment"), None);
        assert_eq!(parse_dotenv_line(""), None);
        assert_eq!(parse_dotenv_line("   "), None);
        assert_eq!(parse_dotenv_line("not an assignment"), None);
        assert_eq!(parse_dotenv_line("1BAD=x"), None);
        assert_eq!(parse_dotenv_line("BAD-KEY=x"), None);
    }

    #[test]
    fn unquotes_round_tripping_our_writer() {
        // Mirrors `destinations::format_env_line` so a value we wrote reads back
        // identically.
        assert_eq!(
            parse_dotenv_line(r#"K="v""#),
            Some(("K".into(), "v".into()))
        );
        assert_eq!(
            parse_dotenv_line(r#"K="a\"b\\c\$d\`e""#),
            Some(("K".into(), r#"a"b\c$d`e"#.into()))
        );
        assert_eq!(
            parse_dotenv_line(r#"K="a\nb\tc""#),
            Some(("K".into(), "a\nb\tc".into()))
        );
        // Single quotes are literal.
        assert_eq!(
            parse_dotenv_line(r#"K='a\nb'"#),
            Some(("K".into(), r#"a\nb"#.into()))
        );
    }

    #[test]
    fn last_assignment_wins_within_a_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "K=first\nK=second\n").unwrap();
        let plan = plan_dotenv_load(dir.path(), |_| false);
        assert_eq!(plan, vec![("K".into(), "second".into(), "the .env file")]);
    }

    #[test]
    fn shell_env_beats_both_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "SHELLVAR=fromenv\nONLYENV=e\n").unwrap();
        fs::write(dir.path().join(".env.local"), "SHELLVAR=fromlocal\n").unwrap();
        let shell: HashSet<&str> = ["SHELLVAR"].into_iter().collect();
        let plan = plan_dotenv_load(dir.path(), |k| shell.contains(k));
        // SHELLVAR is owned by the shell and must not be planned at all.
        assert_eq!(plan, vec![("ONLYENV".into(), "e".into(), "the .env file")]);
    }

    #[test]
    fn dotenv_beats_dotenv_local() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "K=from_env\n").unwrap();
        fs::write(dir.path().join(".env.local"), "K=from_local\nONLYLOCAL=l\n").unwrap();
        let plan = plan_dotenv_load(dir.path(), |_| false);
        assert_eq!(
            plan,
            vec![
                ("K".into(), "from_env".into(), "the .env file"),
                ("ONLYLOCAL".into(), "l".into(), "the .env.local file"),
            ]
        );
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(plan_dotenv_load(dir.path(), |_| false).is_empty());
    }
}
