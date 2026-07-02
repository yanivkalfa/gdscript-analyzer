//! Library core of the `gdscript` CLI — the command dispatcher, exit codes (Playbook §6), color
//! resolution, and the panic hook. `main.rs` is a thin shell that parses args and `process::exit`s
//! the result. The CLI is the only layer that touches the filesystem; everything else flows through
//! [`gdscript_ide`].

pub mod cli;
mod config;
mod engine;
mod lines;
mod report;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use gdscript_base::Severity;

use cli::{Cli, Command, FormatArgs, GlobalArgs, PathsArg};
use config::Config;
use engine::Project;

/// Clean — nothing at or above the fail threshold.
pub const EXIT_OK: i32 = 0;
/// Diagnostics found (or `format --check` would reformat).
pub const EXIT_DIAGNOSTICS: i32 = 1;
/// Usage / config error.
pub const EXIT_USAGE: i32 = 2;
/// Internal error (a caught panic) — matches Rust's default panic exit code.
pub const EXIT_INTERNAL: i32 = 101;

/// Install a panic hook that prints a stable bug-report line before the default hook fires.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!(
            "gdscript: internal error (this is a bug) — please report at \
             https://github.com/yanivkalfa/gdscript-analyzer/issues"
        );
        default(info);
    }));
}

/// Run the parsed CLI; return the process exit code. A usage/config error becomes [`EXIT_USAGE`].
#[must_use]
pub fn run(cli: &Cli) -> i32 {
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("gdscript: {e:#}");
            EXIT_USAGE
        }
    }
}

/// Which diagnostics a command displays.
#[derive(Clone, Copy)]
enum DiagFilter {
    /// Everything (`check`).
    All,
    /// Non-error severities only (`lint`).
    WarningsOnly,
}

fn dispatch(cli: &Cli) -> anyhow::Result<i32> {
    let g = &cli.global;
    match &cli.command {
        Command::Check(p) => {
            let cfg = resolve_config(g, &p.paths)?;
            run_diagnostics(p, g, DiagFilter::All, &cfg)
        }
        Command::Lint(p) => {
            let cfg = resolve_config(g, &p.paths)?;
            run_diagnostics(p, g, DiagFilter::WarningsOnly, &cfg)
        }
        // `symbols` (data) and `format` don't consume the lint config options, but an explicit
        // `--config <bad file>` must still surface as a config error (exit 2) — so validate it.
        Command::Symbols(p) => {
            resolve_config(g, &p.paths)?;
            run_symbols(p, g)
        }
        Command::Format(args) => {
            resolve_config(g, &args.paths.paths)?;
            Ok(run_format(args, g))
        }
    }
}

/// Resolve the analyzer config for a run from the global flags + the first target path (for walk-up
/// discovery). A bad explicit `--config` file is a usage/config error (mapped to exit 2 by [`run`]).
fn resolve_config(g: &GlobalArgs, paths: &[PathBuf]) -> anyhow::Result<Config> {
    let first = paths
        .first()
        .map_or_else(|| Path::new("."), PathBuf::as_path);
    config::resolve(g.no_config, g.config.as_deref(), first)
}

/// `check` / `lint`: load the project, run diagnostics in parallel, emit, and pick the exit code.
fn run_diagnostics(
    paths: &PathsArg,
    g: &GlobalArgs,
    filter: DiagFilter,
    cfg: &Config,
) -> anyhow::Result<i32> {
    let mut project = Project::load(&paths.paths);
    project.set_warning_override(g.warning_override());
    print_load_errors(&project);
    let mut results = project.diagnostics();
    if let DiagFilter::WarningsOnly = filter {
        for fd in &mut results {
            fd.diagnostics
                .retain(|d| !matches!(d.severity, Severity::Error));
        }
    }
    // Warnings fail the run if `--error-on-warning` OR the config's `error_on_warning` is set.
    let error_on_warning = cfg.error_on_warning(g.error_on_warning);
    let failing = results
        .iter()
        .flat_map(|fd| &fd.diagnostics)
        .any(|d| is_failing(d.severity, error_on_warning));
    let total: usize = results.iter().map(|fd| fd.diagnostics.len()).sum();

    let color = g.output_file.is_none() && resolve_color(g.no_color);
    let mut w = open_writer(g)?;
    report::emit(g.format, &results, color, &mut w)?;
    w.flush()?;

    if g.format == cli::Format::Human && !g.quiet {
        if total == 0 {
            eprintln!("No issues found.");
        } else {
            eprintln!("Found {total} issue{}.", if total == 1 { "" } else { "s" });
        }
    }
    Ok(if failing { EXIT_DIAGNOSTICS } else { EXIT_OK })
}

/// `symbols`: dump each file's document outline as JSON (always — it's a data command).
fn run_symbols(paths: &PathsArg, g: &GlobalArgs) -> anyhow::Result<i32> {
    // Document symbols are a per-file outline — no cross-file analysis, no context walk.
    let project = Project::load_shallow(&paths.paths);
    print_load_errors(&project);
    let results = project.symbols();
    let mut w = open_writer(g)?;
    report::emit_symbols(&results, &mut w)?;
    w.flush()?;
    Ok(EXIT_OK)
}

/// `format`: normalize each file via [`gdscript_fmt`]. Default streams to stdout; `--check` reports
/// + exits 1 on any change; `--write` rewrites changed files in place.
fn run_format(args: &FormatArgs, g: &GlobalArgs) -> i32 {
    // Formatting is purely per-file textual — no cross-file analysis, so no project context walk.
    let project = Project::load_shallow(&args.paths.paths);
    print_load_errors(&project);
    let cfg = gdscript_fmt::FmtConfig::default();

    // Default mode streams the formatted text to the output sink (stdout / `--output-file`).
    let mut writer = if args.check || args.write {
        None
    } else {
        match open_writer(g) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("gdscript: {e}");
                return EXIT_USAGE;
            }
        }
    };

    let mut unformatted = 0usize;
    let mut write_errors = 0usize;
    for f in project.targets() {
        let formatted = gdscript_fmt::format(&f.text, &cfg);
        let changed = formatted != *f.text;
        if args.check {
            if changed {
                unformatted += 1;
                if !g.quiet {
                    eprintln!("would reformat: {}", f.display);
                }
            }
        } else if args.write {
            if changed {
                if let Some(p) = &f.path {
                    if let Err(e) = std::fs::write(p, &formatted) {
                        eprintln!("gdscript: cannot write {}: {e}", f.display);
                        write_errors += 1;
                    } else if !g.quiet {
                        eprintln!("formatted: {}", f.display);
                    }
                } else {
                    eprintln!(
                        "gdscript: cannot write {} in place (no on-disk path)",
                        f.display
                    );
                    write_errors += 1;
                }
            }
        } else if let Some(w) = writer.as_mut() {
            let _ = w.write_all(formatted.as_bytes());
        }
    }
    if let Some(w) = writer.as_mut() {
        let _ = w.flush();
    }

    if write_errors > 0 {
        return EXIT_INTERNAL;
    }
    if args.check && unformatted > 0 {
        if !g.quiet {
            eprintln!("{unformatted} file(s) would be reformatted.");
        }
        return EXIT_DIAGNOSTICS;
    }
    EXIT_OK
}

/// A diagnostic that should drive a non-zero exit: an error always, a warning only under
/// `--error-on-warning`.
fn is_failing(sev: Severity, error_on_warning: bool) -> bool {
    matches!(sev, Severity::Error) || (error_on_warning && matches!(sev, Severity::Warning))
}

/// Print unreadable-file problems to stderr (they don't change the exit code on their own).
fn print_load_errors(project: &Project) {
    for e in &project.errors {
        eprintln!("gdscript: cannot read {}: {}", e.display, e.message);
    }
}

/// Resolve whether to colorize, honoring the de-facto env conventions in precedence order:
/// `--no-color`/`NO_COLOR`/`CLICOLOR=0` force off, `CLICOLOR_FORCE` (≠0) forces on, else by tty.
fn resolve_color(no_color: bool) -> bool {
    if no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    // `CLICOLOR=0` explicitly disables (the plain CLICOLOR convention); any other value is advisory.
    if std::env::var("CLICOLOR").is_ok_and(|v| v == "0") {
        return false;
    }
    if std::env::var("CLICOLOR_FORCE").is_ok_and(|v| v != "0") {
        return true;
    }
    io::stdout().is_terminal()
}

/// The output sink: a file (`--output-file`) or stdout, buffered.
fn open_writer(g: &GlobalArgs) -> anyhow::Result<Box<dyn Write>> {
    match &g.output_file {
        Some(path) => {
            let f = std::fs::File::create(path)
                .with_context(|| format!("cannot create output file {}", path.display()))?;
            Ok(Box::new(io::BufWriter::new(f)))
        }
        None => Ok(Box::new(io::BufWriter::new(io::stdout().lock()))),
    }
}
