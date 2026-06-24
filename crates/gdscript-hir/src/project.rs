//! `project.godot` autoload parsing (Playbook §3.M4).
//!
//! `project.godot` is a Godot `ConfigFile` (INI-like: `[section]` headers, `key=value`,
//! typed-Variant values). We do **not** evaluate full Variant values — the analyzer needs only
//! the `[autoload]` section, which is line-oriented `Name="*res://path"`. This is a deliberate
//! minimal scan (not a `VariantParser` port): track the current `[section]`, and within
//! `[autoload]` split each line on the first `=`, take the bare LHS as the autoload name and the
//! dequoted RHS as the resource path. The leading `*` on the path is the **singleton/global**
//! flag (`project_settings.cpp`: `begins_with("*")` → `is_singleton`, then `substr(1)` strips it);
//! a non-`*` autoload is loaded at `/root/Name` but is **not** a global identifier.
//!
//! Pure (`fn(&str) -> Vec<AutoloadEntry>`) and wasm-clean — the host injects the text.

use smol_str::SmolStr;

/// One `[autoload]` entry from `project.godot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoloadEntry {
    /// The autoload (and, when `is_singleton`, the global identifier) name — the bare LHS.
    pub name: SmolStr,
    /// The resource path, with the leading `*` already stripped (e.g. `res://game.gd`).
    pub path: SmolStr,
    /// Whether the entry was `*`-flagged: a **global singleton** (the bare name resolves in code).
    /// A non-singleton autoload is loaded-but-not-global.
    pub is_singleton: bool,
}

/// Parse the `[autoload]` entries from `project.godot` text. Robust to comments, blank lines, and
/// other sections; never panics on malformed input (a bad line is skipped).
#[must_use]
pub fn parse_autoloads(text: &str) -> Vec<AutoloadEntry> {
    let mut entries = Vec::new();
    let mut in_autoload = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        // Skip blanks and `;`-comments (ConfigFile comment marker).
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        // A `[section]` header switches context.
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_autoload = inner.trim() == "autoload";
            continue;
        }
        if !in_autoload {
            continue;
        }
        // `Name="*res://path"` — split on the first `=`.
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // Dequote the value (real files always quote the path); then strip the `*` singleton flag.
        let value = dequote(value.trim());
        let (is_singleton, path) = match value.strip_prefix('*') {
            Some(rest) => (true, rest),
            None => (false, value),
        };
        if path.is_empty() {
            continue;
        }
        entries.push(AutoloadEntry {
            name: SmolStr::new(name),
            path: SmolStr::new(path),
            is_singleton,
        });
    }
    entries
}

/// Strip one layer of matching surrounding quotes (`"…"` / `'…'`), else return as-is.
fn dequote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_singleton_and_strips_star() {
        let e = parse_autoloads("[autoload]\nGame=\"*res://game.gd\"\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "Game");
        assert_eq!(e[0].path, "res://game.gd");
        assert!(e[0].is_singleton);
    }

    #[test]
    fn non_star_is_not_a_singleton() {
        let e = parse_autoloads("[autoload]\nHelper=\"res://helper.gd\"\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "res://helper.gd");
        assert!(!e[0].is_singleton, "no leading * → loaded-but-not-global");
    }

    #[test]
    fn only_the_autoload_section_is_read() {
        let src = "config_version=5\n\
            [application]\n\
            config/name=\"Demo\"\n\
            config/features=PackedStringArray(\"4.6\")\n\
            \n\
            [autoload]\n\
            ; a comment\n\
            Log=\"*res://utils/system_log.gd\"\n\
            Music=\"*res://music.tscn\"\n\
            \n\
            [rendering]\n\
            renderer/rendering_method=\"gl_compatibility\"\n";
        let e = parse_autoloads(src);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "Log");
        assert_eq!(e[0].path, "res://utils/system_log.gd");
        assert!(e[0].is_singleton);
        // A `.tscn` (PackedScene) autoload is captured the same way (typed at resolution time).
        assert_eq!(e[1].name, "Music");
        assert_eq!(e[1].path, "res://music.tscn");
        // The `config/name` line under [application] is NOT mistaken for an autoload.
    }

    #[test]
    fn empty_or_no_autoload_section_is_empty() {
        assert!(parse_autoloads("").is_empty());
        assert!(parse_autoloads("[application]\nconfig/name=\"X\"\n").is_empty());
    }
}
