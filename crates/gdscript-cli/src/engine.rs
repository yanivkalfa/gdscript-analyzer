//! File discovery + the load→fan-out batch engine (Playbook §4/§5).
//!
//! The leverage point: load `project.godot` + **every** discovered `.gd` file into ONE
//! [`AnalysisHost`] (so cross-file resolution — `class_name`, `preload`, autoloads — works), take a
//! cheap [`Analysis`](gdscript_ide::Analysis) snapshot, then fan out the per-file read query over
//! `rayon`. The load phase is serial (the host is single-writer); the read phase is embarrassingly
//! parallel (the snapshot is `Clone + Send` and nothing mutates during reads, so no read cancels).

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gdscript_base::{Diagnostic, DocumentSymbol, FileId};
use gdscript_ide::{AnalysisHost, Change};
use rayon::prelude::*;

use crate::lines::LineIndex;

/// A discovered source file with everything an emitter needs.
#[derive(Debug)]
pub struct SourceFile {
    /// The salsa file handle.
    pub id: FileId,
    /// The path to show in output (project-relative when possible, else as given).
    pub display: String,
    /// The source text (also fed to the host; reused for human snippets + the line index).
    pub text: Arc<str>,
    /// The per-file 1-based position converter. Built for TARGET files only (`Some` iff
    /// `is_target`) — context files are never rendered, and the O(len) scan per file would
    /// otherwise scale every command with project size.
    pub line_index: Option<LineIndex>,
    /// The on-disk path (for `format --write`); `None` for stdin.
    pub path: Option<PathBuf>,
    /// Whether the user asked to check this file. When a `project.godot` root is found, the whole
    /// project is loaded so cross-file resolution (`class_name`, autoloads, `UNDEFINED_*`) is
    /// sound; the extra files are CONTEXT (`false`) — analyzed for resolution, never reported.
    pub is_target: bool,
}

/// A file we could not read (surfaced as a usage-level problem, not a diagnostic).
#[derive(Debug)]
pub struct LoadError {
    /// The path we failed on.
    pub display: String,
    /// Why.
    pub message: String,
}

/// The loaded world: the host, the files (in discovery order), and any unreadable paths.
#[derive(Debug)]
pub struct Project {
    host: AnalysisHost,
    /// Discovered files.
    pub files: Vec<SourceFile>,
    /// Files that failed to read.
    pub errors: Vec<LoadError>,
}

/// Per-file diagnostics, ready to emit (the file + its sorted diagnostics).
#[derive(Debug)]
pub struct FileDiagnostics<'a> {
    /// The file these belong to.
    pub file: &'a SourceFile,
    /// Diagnostics, sorted by start offset.
    pub diagnostics: Vec<Diagnostic>,
}

/// Per-file document symbols.
#[derive(Debug)]
pub struct FileSymbols<'a> {
    /// The file these belong to.
    pub file: &'a SourceFile,
    /// The document outline.
    pub symbols: Vec<DocumentSymbol>,
}

impl Project {
    /// Discover the `.gd` files under `targets`, find the project root, and load everything into one
    /// host. `targets` may be files, directories, or `-` (stdin, read as a single anonymous file).
    #[must_use]
    pub fn load(targets: &[PathBuf]) -> Self {
        Self::load_inner(targets, /*deep=*/ true)
    }

    /// Load ONLY the targets — no whole-project context walk, no completeness claim. For the
    /// purely per-file commands (`format`, `symbols`) that need zero cross-file analysis: a
    /// single-file format must not scale with project size.
    #[must_use]
    pub fn load_shallow(targets: &[PathBuf]) -> Self {
        Self::load_inner(targets, /*deep=*/ false)
    }

    fn load_inner(targets: &[PathBuf], deep: bool) -> Self {
        // Each entry is `(display, res_path, text)`. The **display path is derived from the
        // `res://` path** (the single source of truth) so the two never diverge: when a file
        // resolves under the project root both are `Some`/relative; otherwise both fall back to the
        // raw path together (Hunt #2/#3 — a prior split derivation could disagree if root vs file
        // canonicalization differed).
        // `(display, res_path, text, on-disk path, is_target)`.
        type Discovered = (String, Option<String>, Arc<str>, Option<PathBuf>, bool);
        let mut discovered: Vec<Discovered> = Vec::new();
        let mut errors = Vec::new();
        let mut seen = BTreeSet::new();

        // Stdin (`-`) is a single anonymous file with no `res://` path (no project anchor).
        let stdin_requested = targets.iter().any(|t| t.as_os_str() == "-");
        let fs_targets: Vec<&PathBuf> = targets.iter().filter(|t| t.as_os_str() != "-").collect();

        // Each filesystem target's project root (its nearest `project.godot` ancestor). The FIRST
        // target's root anchors `res://` paths + the context walk; the completeness claim below
        // additionally requires that EVERY target lives under that same root — a target from a
        // second project would otherwise be judged "complete" without its own siblings loaded.
        let roots: Vec<Option<PathBuf>> = fs_targets
            .iter()
            .map(|p| find_project_root(p.as_path()))
            .collect();
        let root = roots.first().cloned().flatten();
        let single_root = root.is_some() && roots.iter().all(|r| r.as_deref() == root.as_deref());

        if stdin_requested {
            let mut buf = String::new();
            match std::io::stdin().read_to_string(&mut buf) {
                Ok(_) => discovered.push(("<stdin>".to_owned(), None, Arc::from(buf), None, true)),
                Err(e) => errors.push(LoadError {
                    display: "<stdin>".into(),
                    message: e.to_string(),
                }),
            }
        }

        let mut discover = |path: PathBuf, is_target: bool| {
            let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(canon.clone()) {
                return; // dedup files reached via overlapping targets / the context walk
            }
            let res_path = root.as_deref().and_then(|r| res_path_for(r, &canon));
            let display = res_path
                .as_deref()
                .map_or_else(|| raw_display(&path), res_display);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    // GDScript is UTF-8; decode lossily so a stray byte never aborts the run.
                    let text: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).into_owned());
                    discovered.push((display, res_path, text, Some(path), is_target));
                }
                Err(e) => errors.push(LoadError {
                    display,
                    message: e.to_string(),
                }),
            }
        };

        // Godot-faithful discovery (see `godot_walk`): `.gitignore` is deliberately NOT honored
        // and `.gdignore` is Godot's empty MARKER (skip the whole directory) — the loaded set
        // must be exactly what Godot itself would compile, or the completeness claim is a lie.
        let mut unanalyzable = false;
        let root_canon = root
            .as_deref()
            .map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()));
        let mut root_walked = false;
        for target in &fs_targets {
            let walk = godot_walk(target);
            // A target that IS the project root already covers the whole tree — its walk doubles
            // as the context walk (no second traversal, and its unanalyzable flag is the root's).
            if root_canon
                .as_deref()
                .is_some_and(|rc| target.canonicalize().is_ok_and(|c| c == rc))
            {
                root_walked = true;
                unanalyzable |= walk.unanalyzable;
            }
            for path in walk.gd_files {
                discover(path, true);
            }
        }
        // With a project root found, ALSO load every other `.gd` under it as CONTEXT (dedup'd
        // against the targets): cross-file resolution — `class_name` globals, autoload scripts,
        // and the whole-project absence proof behind `UNDEFINED_*` — needs the full graph in ONE
        // host. Context files are analyzed for resolution only, never reported. Shallow loads
        // (per-file commands) skip this entirely.
        if deep
            && !root_walked
            && let Some(r) = root.as_deref()
        {
            let walk = godot_walk(r);
            unanalyzable |= walk.unanalyzable;
            for path in walk.gd_files {
                discover(path, false);
            }
        }

        // ---- load: serial single-writer phase ----
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let mut files = Vec::with_capacity(discovered.len());
        for (i, (display, res_path, text, path, is_target)) in discovered.into_iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
            change.change_file(id, Arc::clone(&text));
            if let Some(res) = &res_path {
                change.set_file_path(id, res.clone());
            }
            files.push(SourceFile {
                id,
                display,
                line_index: is_target.then(|| LineIndex::new(&text)),
                text,
                path,
                is_target,
            });
        }
        // Feed `project.godot` so autoload resolution lights up.
        if let Some(cfg) = root.as_deref().and_then(read_project_godot) {
            change.set_project_config(cfg);
        }
        // The workspace-completeness claim arming the absence-based `UNDEFINED_*` diagnostics.
        // Claimed ONLY when this load provably saw everything Godot would compile:
        // - a deep load with a project root, whose whole tree was walked (Godot-faithfully);
        // - every filesystem target under that SAME root (a second project's targets would be
        //   judged against the wrong graph);
        // - no stdin (its content has no project anchor);
        // - no `.gdextension` / C# sources (runtime-registered classes are invisible to us);
        // - zero read failures (an unreadable file's `class_name`s are missing from the view).
        let complete =
            deep && single_root && !stdin_requested && !unanalyzable && errors.is_empty();
        change.set_workspace_complete(complete);
        host.apply_change(change);

        Self {
            host,
            files,
            errors,
        }
    }

    /// The files the user asked to check (context files loaded for cross-file resolution are
    /// excluded) — what every reporting command iterates.
    pub fn targets(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter().filter(|f| f.is_target)
    }

    /// Apply a CLI `--strict` / `--engine-defaults` warning-strictness override before any
    /// `diagnostics()` snapshot. A plain host field (not a salsa input), so it only affects the
    /// downstream gate, never re-running inference.
    pub fn set_warning_override(&mut self, ov: gdscript_ide::WarningOverride) {
        self.host.set_warning_override(ov);
    }

    /// Run `diagnostics(file)` for every TARGET file (context files resolve, never report), in
    /// parallel over per-worker snapshot clones. Results are sorted by `(display path, start
    /// offset)` for deterministic output regardless of scheduling. `Analysis` is `Send + Clone`
    /// but not `Sync`, so `map_with` hands each rayon worker its own cheap clone (the
    /// load→fan-out model) rather than sharing one snapshot.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<FileDiagnostics<'_>> {
        let mut out: Vec<FileDiagnostics<'_>> = self
            .files
            .par_iter()
            .filter(|f| f.is_target)
            .map_with(self.host.analysis(), |analysis, file| {
                // No concurrent writes during the read phase ⇒ the snapshot never cancels.
                let mut diagnostics = analysis.diagnostics(file.id).unwrap_or_default();
                diagnostics.sort_by_key(|d| (d.range.start, d.range.end));
                FileDiagnostics { file, diagnostics }
            })
            .collect();
        // Sort by display path; break ties on `FileId` (discovery order) so output is fully
        // deterministic even if two files share a display string (Hunt #4 — explicit, not relying
        // on the stable-sort + ordered-collect invariant alone).
        out.sort_by(|a, b| {
            a.file
                .display
                .cmp(&b.file.display)
                .then_with(|| a.file.id.0.cmp(&b.file.id.0))
        });
        out
    }

    /// Run `document_symbols(file)` for every TARGET file, in parallel. Sorted by display path.
    #[must_use]
    pub fn symbols(&self) -> Vec<FileSymbols<'_>> {
        let mut out: Vec<FileSymbols<'_>> = self
            .files
            .par_iter()
            .filter(|f| f.is_target)
            .map_with(self.host.analysis(), |analysis, file| FileSymbols {
                file,
                symbols: analysis.document_symbols(file.id).unwrap_or_default(),
            })
            .collect();
        out.sort_by(|a, b| {
            a.file
                .display
                .cmp(&b.file.display)
                .then_with(|| a.file.id.0.cmp(&b.file.id.0))
        });
        out
    }
}

/// One Godot-faithful discovery pass: the `.gd` files plus whether the tree contains sources that
/// can register globals the analyzer cannot see (see [`is_unanalyzable`]).
struct GodotWalk {
    gd_files: Vec<PathBuf>,
    unanalyzable: bool,
}

/// Walk a target the way GODOT's filesystem scan does — the loaded set must be exactly what the
/// engine itself would compile, or the workspace-completeness claim behind `UNDEFINED_*` is a lie:
/// - `.gitignore` is deliberately NOT honored (Godot compiles gitignored scripts; a gitignored
///   `addons/` full of `class_name`s is the classic false-positive trap);
/// - Godot's `.gdignore` is an empty MARKER file — a directory containing one is skipped
///   ENTIRELY (it is not a gitignore-style pattern file);
/// - dot-directories (`.git`, `.godot`, …) are skipped, like the editor's scan.
///
/// A target that is itself a file is yielded directly.
fn godot_walk(target: &Path) -> GodotWalk {
    let mut out = GodotWalk {
        gd_files: Vec::new(),
        unanalyzable: false,
    };
    if target.is_file() {
        if is_gd(target) {
            out.gd_files.push(target.to_path_buf());
        } else if is_unanalyzable(target) {
            out.unanalyzable = true;
        }
        return out;
    }
    godot_walk_dir(target, &mut out);
    out
}

fn godot_walk_dir(dir: &Path, out: &mut GodotWalk) {
    if dir.join(".gdignore").is_file() {
        return; // Godot excludes this whole directory
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // Sort for a deterministic discovery order — FileId order feeds the first-wins registries.
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            if !e.file_name().to_string_lossy().starts_with('.') {
                godot_walk_dir(&p, out);
            }
        } else if is_gd(&p) {
            out.gd_files.push(p);
        } else if is_unanalyzable(&p) {
            out.unanalyzable = true;
        }
    }
}

/// Whether a path has a `.gd` extension (case-insensitive).
fn is_gd(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gd"))
}

/// A source that can register globals the analyzer cannot see — a GDExtension (`.gdextension`) or
/// C# (`.csproj`/`.sln`/`.cs`). Their runtime-registered classes make "defined nowhere"
/// unprovable, so their presence withholds the workspace-completeness claim.
fn is_unanalyzable(p: &Path) -> bool {
    p.extension().is_some_and(|e| {
        e.eq_ignore_ascii_case("gdextension")
            || e.eq_ignore_ascii_case("csproj")
            || e.eq_ignore_ascii_case("sln")
            || e.eq_ignore_ascii_case("cs")
    })
}

/// Walk up from `start` (a file or dir) to the nearest ancestor directory containing `project.godot`.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    // Resolve to absolute so `..`/relative targets walk a real chain.
    dir = dir.canonicalize().unwrap_or(dir);
    loop {
        if dir.join("project.godot").is_file() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// The `res://`-relative path of `file` under project `root`, with `/` separators, or `None` when
/// `file` is not under `root`.
fn res_path_for(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let mut s = String::from("res://");
    for (i, comp) in rel.components().enumerate() {
        if i > 0 {
            s.push('/');
        }
        s.push_str(&comp.as_os_str().to_string_lossy());
    }
    Some(s)
}

/// The display path derived from a `res://` path: drop the `res://` scheme (the remainder is already
/// project-relative with `/` separators). Keeps `display` and the `res://` path in lockstep.
fn res_display(res_path: &str) -> String {
    res_path
        .strip_prefix("res://")
        .unwrap_or(res_path)
        .to_owned()
}

/// The fallback display for a file with no `res://` path (no project root, or outside it): the path
/// as discovered, normalized to `/` separators for cross-platform-stable output.
fn raw_display(file: &Path) -> String {
    file.to_string_lossy().replace('\\', "/")
}

/// Read a project's `project.godot` text, if present + readable.
fn read_project_godot(root: &Path) -> Option<Arc<str>> {
    std::fs::read(root.join("project.godot"))
        .ok()
        .map(|b| Arc::from(String::from_utf8_lossy(&b).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn res_display_drops_the_scheme() {
        assert_eq!(res_display("res://scripts/main.gd"), "scripts/main.gd");
        assert_eq!(res_display("res://a.gd"), "a.gd");
        assert_eq!(res_display("no-scheme"), "no-scheme"); // defensive fallthrough
    }

    #[test]
    fn raw_display_normalizes_separators() {
        // A backslash path collapses to forward slashes on every platform (deterministic output).
        assert_eq!(raw_display(Path::new("a\\b\\c.gd")), "a/b/c.gd");
    }
}
