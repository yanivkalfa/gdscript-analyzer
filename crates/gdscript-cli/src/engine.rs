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
use ignore::WalkBuilder;
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
    /// The per-file 1-based position converter.
    pub line_index: LineIndex,
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
        let mut discovered: Vec<(PathBuf, Arc<str>, Option<String>)> = Vec::new();
        let mut errors = Vec::new();
        let mut seen = BTreeSet::new();

        // Stdin (`-`) is a single anonymous file with no `res://` path (no project anchor).
        let stdin_requested = targets.iter().any(|t| t.as_os_str() == "-");
        let fs_targets: Vec<&PathBuf> = targets.iter().filter(|t| t.as_os_str() != "-").collect();

        // The project root: the nearest `project.godot` ancestor of the first filesystem target.
        let root = fs_targets
            .first()
            .map(|p| p.as_path())
            .and_then(find_project_root);

        if stdin_requested {
            let mut buf = String::new();
            match std::io::stdin().read_to_string(&mut buf) {
                Ok(_) => discovered.push((PathBuf::from("<stdin>"), Arc::from(buf), None)),
                Err(e) => errors.push(LoadError {
                    display: "<stdin>".into(),
                    message: e.to_string(),
                }),
            }
        }

        for target in fs_targets {
            for path in walk_gd_files(target) {
                let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
                if !seen.insert(canon.clone()) {
                    continue; // dedup files reached via overlapping targets
                }
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        // GDScript is UTF-8; decode lossily so a stray byte never aborts the run.
                        let text: Arc<str> =
                            Arc::from(String::from_utf8_lossy(&bytes).into_owned());
                        let res_path = root.as_deref().and_then(|r| res_path_for(r, &canon));
                        discovered.push((path, text, res_path));
                    }
                    Err(e) => errors.push(LoadError {
                        display: display_path(root.as_deref(), &path),
                        message: e.to_string(),
                    }),
                }
            }
        }

        // ---- load: serial single-writer phase ----
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let mut files = Vec::with_capacity(discovered.len());
        for (i, (path, text, res_path)) in discovered.into_iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
            change.change_file(id, Arc::clone(&text));
            if let Some(res) = &res_path {
                change.set_file_path(id, res.clone());
            }
            files.push(SourceFile {
                id,
                display: display_path(root.as_deref(), &path),
                line_index: LineIndex::new(&text),
                text,
            });
        }
        // Feed `project.godot` so autoload resolution lights up.
        if let Some(cfg) = root.as_deref().and_then(read_project_godot) {
            change.set_project_config(cfg);
        }
        host.apply_change(change);

        Self {
            host,
            files,
            errors,
        }
    }

    /// Run `diagnostics(file)` for every file, in parallel over per-worker snapshot clones. Results
    /// are sorted by `(display path, start offset)` for deterministic output regardless of
    /// scheduling. `Analysis` is `Send + Clone` but not `Sync`, so `map_with` hands each rayon
    /// worker its own cheap clone (the load→fan-out model) rather than sharing one snapshot.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<FileDiagnostics<'_>> {
        let mut out: Vec<FileDiagnostics<'_>> = self
            .files
            .par_iter()
            .map_with(self.host.analysis(), |analysis, file| {
                // No concurrent writes during the read phase ⇒ the snapshot never cancels.
                let mut diagnostics = analysis.diagnostics(file.id).unwrap_or_default();
                diagnostics.sort_by_key(|d| (d.range.start, d.range.end));
                FileDiagnostics { file, diagnostics }
            })
            .collect();
        out.sort_by(|a, b| a.file.display.cmp(&b.file.display));
        out
    }

    /// Run `document_symbols(file)` for every file, in parallel. Sorted by display path.
    #[must_use]
    pub fn symbols(&self) -> Vec<FileSymbols<'_>> {
        let mut out: Vec<FileSymbols<'_>> = self
            .files
            .par_iter()
            .map_with(self.host.analysis(), |analysis, file| FileSymbols {
                file,
                symbols: analysis.document_symbols(file.id).unwrap_or_default(),
            })
            .collect();
        out.sort_by(|a, b| a.file.display.cmp(&b.file.display));
        out
    }
}

/// Walk a target path for `.gd` files, honoring `.gitignore` + a custom `.gdignore`. A target that
/// is itself a file is yielded directly (even without a `.gd` extension would be skipped); a
/// directory is walked.
fn walk_gd_files(target: &Path) -> Vec<PathBuf> {
    if target.is_file() {
        return if is_gd(target) {
            vec![target.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(target);
    builder.add_custom_ignore_filename(".gdignore");
    for entry in builder.build().flatten() {
        let p = entry.path();
        if p.is_file() && is_gd(p) {
            out.push(p.to_path_buf());
        }
    }
    out
}

/// Whether a path has a `.gd` extension (case-insensitive).
fn is_gd(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gd"))
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

/// A display path: project-relative (with `/` separators) when under `root`, else the path as given.
fn display_path(root: Option<&Path>, file: &Path) -> String {
    if let Some(root) = root {
        let canon = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        if let Ok(rel) = canon.strip_prefix(root) {
            return rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
        }
    }
    file.to_string_lossy().into_owned()
}

/// Read a project's `project.godot` text, if present + readable.
fn read_project_godot(root: &Path) -> Option<Arc<str>> {
    std::fs::read(root.join("project.godot"))
        .ok()
        .map(|b| Arc::from(String::from_utf8_lossy(&b).into_owned()))
}
