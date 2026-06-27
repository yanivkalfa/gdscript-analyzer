//! Analyzer configuration discovery + plumbing (Playbook §4).
//!
//! A `gdscript-analyzer.toml` is discovered by walking **up** from the target to the filesystem root
//! (nearest wins); `--config <file>` overrides discovery with an explicit file; `--config key=value`
//! applies an inline override on top of discovery; `--no-config` / `--isolated` ignores all config.
//!
//! Phase 5 ships the **discovery + plumbing**; the option set the config carries is intentionally
//! minimal — the warning-gating taxonomy and the formatter options are Phase 6. Today it carries
//! `error_on_warning` (a project default for the `--error-on-warning` flag). Fields are `Option` so
//! discovery / file / inline override merge cleanly (a later layer overrides only the keys it sets),
//! and unknown keys are ignored (forward-compatible with a Phase-6 config on a Phase-5 binary).

use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

/// The analyzer config (a `gdscript-analyzer.toml`). All fields optional for layered merging.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Treat warnings as failures (exit 1) by default. The CLI `--error-on-warning` flag ORs on top,
    /// so `true` here can't be undone by omitting the flag — matching "config sets the project floor".
    pub error_on_warning: Option<bool>,
}

impl Config {
    /// Overlay `other`'s set fields onto `self` (later wins; `None` never clobbers).
    fn merge(&mut self, other: &Config) {
        if other.error_on_warning.is_some() {
            self.error_on_warning = other.error_on_warning;
        }
    }

    /// Parse a config from TOML text.
    fn parse(text: &str) -> anyhow::Result<Config> {
        toml::from_str(text).map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Load an explicit config file (`--config <file>`). Errors (missing/unreadable/malformed) are
    /// surfaced to the caller as a usage/config error (exit 2).
    pub fn from_file(path: &Path) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    /// Discover a `gdscript-analyzer.toml` by walking up from `target` (a file or dir) to the root.
    /// A malformed discovered config falls back to the default (never aborts a run — discovery is
    /// best-effort, unlike an explicit `--config` file which must be valid).
    #[must_use]
    pub fn discover(target: &Path) -> Config {
        let mut dir = if target.is_file() {
            target.parent().map(Path::to_path_buf)
        } else {
            Some(target.to_path_buf())
        };
        dir = dir.map(|d| d.canonicalize().unwrap_or(d));
        while let Some(d) = dir {
            let candidate = d.join("gdscript-analyzer.toml");
            if candidate.is_file()
                && let Ok(text) = std::fs::read_to_string(&candidate)
                && let Ok(cfg) = Self::parse(&text)
            {
                return cfg;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Config::default()
    }

    /// Apply an inline `key=value` override (which is itself valid TOML) on top of `self`.
    pub fn apply_inline(&mut self, kv: &str) -> anyhow::Result<()> {
        let parsed =
            Self::parse(kv).with_context(|| format!("invalid --config override '{kv}'"))?;
        self.merge(&parsed);
        Ok(())
    }

    /// The effective "warnings fail" decision: the config default OR the `--error-on-warning` flag.
    #[must_use]
    pub fn error_on_warning(&self, flag: bool) -> bool {
        self.error_on_warning.unwrap_or(false) || flag
    }
}

/// Resolve the effective config for a run from the global flags + the first target path.
/// `--no-config` short-circuits to defaults; an explicit `--config <file>` replaces discovery; an
/// inline `--config key=value` layers over discovery. An unreadable/malformed explicit file is an
/// error (exit 2); discovery failures degrade to defaults.
pub fn resolve(
    no_config: bool,
    config_arg: Option<&str>,
    first_target: &Path,
) -> anyhow::Result<Config> {
    if no_config {
        return Ok(Config::default());
    }
    match config_arg {
        // Inline `key=value` (contains `=` and isn't an existing file) → discovery + the override.
        Some(s) if s.contains('=') && !Path::new(s).is_file() => {
            let mut cfg = Config::discover(first_target);
            cfg.apply_inline(s)?;
            Ok(cfg)
        }
        // An explicit file replaces discovery.
        Some(s) => Config::from_file(Path::new(s)),
        // No `--config` → walk-up discovery.
        None => Ok(Config::discover(first_target)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_on_warning() {
        let c = Config::parse("error_on_warning = true").unwrap();
        assert_eq!(c.error_on_warning, Some(true));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // forward-compat: a Phase-6 option on a Phase-5 binary must not error.
        let c = Config::parse("error_on_warning = true\nfuture_option = 42\n").unwrap();
        assert_eq!(c.error_on_warning, Some(true));
    }

    #[test]
    fn inline_override_layers_over_discovery() {
        let mut c = Config {
            error_on_warning: Some(false),
        };
        c.apply_inline("error_on_warning = true").unwrap();
        assert_eq!(c.error_on_warning, Some(true));
    }

    #[test]
    fn effective_is_config_or_flag() {
        assert!(Config::default().error_on_warning(true)); // flag forces on
        assert!(
            Config {
                error_on_warning: Some(true)
            }
            .error_on_warning(false)
        ); // config forces on
        assert!(!Config::default().error_on_warning(false)); // neither
    }

    #[test]
    fn malformed_inline_is_an_error() {
        let mut c = Config::default();
        assert!(c.apply_inline("this is not = valid = toml").is_err());
    }

    #[test]
    fn no_config_resolves_to_default() {
        let c = resolve(true, Some("error_on_warning=true"), Path::new(".")).unwrap();
        assert_eq!(c.error_on_warning, None);
    }
}
