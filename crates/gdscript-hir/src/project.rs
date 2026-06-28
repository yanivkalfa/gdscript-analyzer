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

use crate::warnings::{WarnLevel, WarningCode, WarningSettings};

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

/// Parse the Godot engine `(major, minor)` version from `project.godot`'s `[application]`
/// `config/features=PackedStringArray("4.3", "Forward Plus", …)` line. Godot writes the engine
/// version as the first `<major>.<minor>` entry of that array; the rest are rendering/feature tags.
/// Returns the first version-shaped entry, or `None` if the line is absent or carries none. A
/// deliberate minimal scan (not a `VariantParser` port); robust to malformed input (never panics).
#[must_use]
pub fn parse_engine_version(text: &str) -> Option<(u32, u32)> {
    let mut in_application = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_application = inner.trim() == "application";
            continue;
        }
        if !in_application {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "config/features" {
            continue;
        }
        // `PackedStringArray("4.3", "Forward Plus")` → the parenthesized list (tolerate a bare
        // quoted value, and the Godot-3.x `PoolStringArray` name, defensively).
        let value = value.trim();
        let inner = value
            .strip_prefix("PackedStringArray(")
            .or_else(|| value.strip_prefix("PoolStringArray("))
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(value);
        return inner
            .split(',')
            .find_map(|part| parse_major_minor(dequote(part.trim())));
    }
    None
}

/// Parse the `debug/gdscript/warnings/*` settings from `project.godot` into a [`WarningSettings`],
/// starting from the engine default for `engine`. Keys live under `[debug]` as
/// `gdscript/warnings/<tail>` (Godot groups a setting by its first path segment). `<tail>` is
/// `enable` / `treat_warnings_as_errors` / `exclude_addons` (bools) or a code's lowercased
/// setting-name mapped to a `0|1|2` (Ignore/Warn/Error) level. A deliberate minimal scan (not a
/// `VariantParser` port); robust to malformed input (a bad line is skipped). `directory_rules`
/// (Godot master, a typed-Variant dict) is not parsed — see `TECH_DEBT.md`.
#[must_use]
pub fn parse_warning_settings(text: &str, engine: (u32, u32)) -> WarningSettings {
    let mut settings = WarningSettings::engine_default(engine);
    let mut in_debug = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_debug = inner.trim() == "debug";
            continue;
        }
        if !in_debug {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(tail) = key.trim().strip_prefix("gdscript/warnings/") else {
            continue;
        };
        let value = dequote(value.trim());
        match tail {
            "enable" => {
                if let Some(b) = parse_bool(value) {
                    settings.enabled = b;
                }
            }
            "treat_warnings_as_errors" => {
                if let Some(b) = parse_bool(value) {
                    settings.treat_as_errors = b;
                }
            }
            "exclude_addons" => {
                if let Some(b) = parse_bool(value) {
                    settings.exclude_addons = b;
                }
            }
            _ => {
                if let Some(code) = WarningCode::from_setting_name(tail)
                    && let Some(level) = parse_warn_level(value)
                {
                    settings.per_code.insert(code, level);
                }
            }
        }
    }
    settings
}

/// Parse a Godot `ConfigFile` boolean (`true`/`false`), or `None`.
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a warning level: the `0|1|2` int Godot 4.x writes, tolerating a legacy `true`/`false`.
fn parse_warn_level(s: &str) -> Option<WarnLevel> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u32>() {
        return WarnLevel::from_int(n);
    }
    match s {
        "true" => Some(WarnLevel::Warn),
        "false" => Some(WarnLevel::Ignore),
        _ => None,
    }
}

/// Parse a `<major>.<minor>` string (ignoring any trailing `.patch`) into `(major, minor)`.
/// `None` for any non-numeric or single-component string (e.g. a feature tag like `"Vulkan"`).
fn parse_major_minor(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
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

    #[test]
    fn parses_engine_version_from_config_features() {
        let src = "config_version=5\n\
            [application]\n\
            config/name=\"Demo\"\n\
            config/features=PackedStringArray(\"4.3\", \"Forward Plus\")\n";
        assert_eq!(parse_engine_version(src), Some((4, 3)));
    }

    #[test]
    fn engine_version_picks_the_version_shaped_entry_anywhere_in_the_array() {
        // The version need not be first; feature tags (rendering, etc.) are skipped.
        let src = "[application]\nconfig/features=PackedStringArray(\"Forward Plus\", \"4.6\", \"Mobile\")\n";
        assert_eq!(parse_engine_version(src), Some((4, 6)));
    }

    #[test]
    fn engine_version_ignores_patch_and_tolerates_bare_value() {
        assert_eq!(
            parse_engine_version("[application]\nconfig/features=PackedStringArray(\"4.2.1\")\n"),
            Some((4, 2)),
        );
        assert_eq!(
            parse_engine_version("[application]\nconfig/features=\"4.5\"\n"),
            Some((4, 5)),
        );
    }

    #[test]
    fn engine_version_none_when_absent_or_no_version_entry() {
        assert_eq!(parse_engine_version(""), None);
        assert_eq!(
            parse_engine_version("[application]\nconfig/name=\"X\"\n"),
            None
        );
        // config/features outside [application] is not the engine version.
        assert_eq!(
            parse_engine_version("[rendering]\nconfig/features=PackedStringArray(\"4.6\")\n"),
            None,
        );
        // A features array with no version-shaped entry → None (not a panic).
        assert_eq!(
            parse_engine_version("[application]\nconfig/features=PackedStringArray(\"Vulkan\")\n"),
            None,
        );
    }
}
