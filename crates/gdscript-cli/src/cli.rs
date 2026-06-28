//! The clap v4 command tree (Playbook §1). `check`/`lint`/`symbols`/`format`, with the
//! output/config flags global so they may appear before or after the subcommand (Ruff/Biome shape).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `gdscript` — a standalone GDScript analyzer CLI (no editor, no Godot).
#[derive(Debug, Parser)]
#[command(
    name = "gdscript",
    version,
    about = "GDScript analyzer — check / lint / format / symbols for CI and pre-commit.",
    long_about = None,
)]
pub struct Cli {
    /// The subcommand.
    #[command(subcommand)]
    pub command: Command,
    /// Flags shared by every subcommand.
    #[command(flatten)]
    pub global: GlobalArgs,
}

/// Flags that apply to every subcommand (declared `global` so they parse in either position).
#[derive(Debug, Args)]
// A CLI flag struct is naturally flag-heavy; the "state machine" refactor clippy suggests does not
// fit independent boolean options.
#[allow(clippy::struct_excessive_bools, reason = "independent CLI flags")]
pub struct GlobalArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human, global = true)]
    pub format: Format,
    /// Write output to this file instead of stdout.
    #[arg(long, value_name = "PATH", global = true)]
    pub output_file: Option<PathBuf>,
    /// A config file path, or an inline `KEY=VALUE` override (Phase-5: discovery/plumbing only).
    #[arg(long, value_name = "FILE-OR-KEY=VALUE", global = true)]
    pub config: Option<String>,
    /// Ignore all configuration files.
    #[arg(long, alias = "isolated", global = true)]
    pub no_config: bool,
    /// Exit non-zero when warnings are present (default: only errors fail).
    #[arg(long, global = true)]
    pub error_on_warning: bool,
    /// Suppress the human summary line.
    #[arg(long, short, global = true)]
    pub quiet: bool,
    /// Disable colored output (also honors `NO_COLOR` / non-tty).
    #[arg(long, global = true)]
    pub no_color: bool,
}

/// The output format for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Format {
    /// Colored, rustc/Ruff-style `path:line:col: severity[CODE] message` (default).
    #[default]
    Human,
    /// A stable JSON array of diagnostics (byte range + 1-based line:col).
    Json,
    /// GitHub Actions workflow-command annotations.
    Github,
    /// SARIF 2.1.0 (GitHub code scanning).
    Sarif,
    /// reviewdog rdjson (1-based UTF-8 byte columns).
    Rdjson,
}

/// Positional paths (files, directories, or `-` for stdin); defaults to the current directory.
#[derive(Debug, Args)]
pub struct PathsArg {
    /// Files, directories, or `-` (stdin).
    #[arg(default_value = ".", value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}

/// `format` options.
#[derive(Debug, Args)]
pub struct FormatArgs {
    /// Paths to format.
    #[command(flatten)]
    pub paths: PathsArg,
    /// Report whether files are formatted (exit 1 if not) without writing.
    #[arg(long)]
    pub check: bool,
    /// Rewrite files in place.
    #[arg(long)]
    pub write: bool,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Parse + type diagnostics across the project (the CI workhorse).
    Check(PathsArg),
    /// Diagnostics filtered to the warning/lint subset (non-error severities).
    Lint(PathsArg),
    /// Format GDScript (normalize indentation + whitespace). `--check` reports without writing;
    /// `--write` rewrites in place; default streams the formatted text to stdout.
    Format(FormatArgs),
    /// Dump each file's document symbols as JSON.
    Symbols(PathsArg),
}
