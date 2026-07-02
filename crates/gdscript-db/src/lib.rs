//! `gdscript-db` — the input layer for the analyzer.
//!
//! > **Internal layer (not a stable API).** Depend on [`gdscript-ide`](https://docs.rs/gdscript-ide) (the public surface); the items here
//! > may change between releases.
//!
//! Holds the virtual file system (`FileId` → text, always injected — never `std::fs`), the
//! project model, and (from Phase 3) the **salsa** query graph: `#[salsa::input]`s set via
//! `apply_change`, `#[salsa::tracked]` derived queries, durability tiers. The Phase-0/1/2
//! plain VFS map + reparse-on-change is being replaced here, localized behind the unchanged
//! `gdscript-ide` public API (Playbook §3.M0).
//!
//! Crate boundary: `gdscript-db` is the *base* of the salsa stack — it owns the [`Db`] trait,
//! the inputs, and the [`parse`] query (it may depend on `gdscript-syntax`, never on
//! `gdscript-hir`). The higher queries (`item_tree`, `analyze_file`) live in `gdscript-hir`,
//! which depends on this crate for `&dyn Db`. This one-way layering is what avoids a
//! `db ↔ hir` dependency cycle.
//!
//! `FileId` is deliberately **not** a salsa input. The `FileId → FileText` mapping is a side
//! table ([`Files`]) the database owns, mirroring rust-analyzer's `base-db`: `FileId`s are
//! assigned by the client/loader and stay opaque ids, while the salsa input is the *text*.
//!
//! Must build for `wasm32` (single-threaded; salsa with `default-features = false`).
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use gdscript_api::EngineApi;
use gdscript_base::FileId;
use gdscript_syntax::Parse;
use rustc_hash::FxBuildHasher;
use salsa::{Durability, Setter};

/// The database trait `gdscript-hir` / `gdscript-ide` depend on. `#[salsa::db]` on the *trait*
/// makes it a salsa supertrait, so any `&dyn Db` upcasts to `&dyn salsa::Database` and every
/// `#[salsa::tracked]` free function downstream can take `db: &dyn Db`.
/// A host/CLI-level override of the warning-strictness baseline `type_diagnostics` resolves against
/// (regardless of `project.godot` presence). A plain (non-salsa) per-session policy knob: it is read
/// **only** inside the non-tracked `type_diagnostics`, so it never enters the salsa query graph and
/// cannot break the W1 firewall (a warning-level change must never re-run inference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WarningOverride {
    /// Auto-select by project presence (the default): standalone ⇒ strict, project ⇒ engine defaults.
    #[default]
    None,
    /// Force the strict baseline (the opt-in group promoted to WARN) even with a `project.godot`.
    Strict,
    /// Force Godot's engine defaults (the opt-in group stays IGNORE) even in standalone mode.
    EngineDefaults,
}

#[salsa::db]
pub trait Db: salsa::Database {
    /// The text input for `file`, or `None` if no text has been set for it.
    fn file_text(&self, file: FileId) -> Option<FileText>;
    /// The bundled engine model, or `None` on `wasm32` (no embedded blob — the host wires the
    /// fetched blob in via `EngineApi::from_bytes` in Phase 5).
    fn engine(&self) -> Option<&'static EngineApi>;
    /// The project's file set, or `None` before any file has been applied. Project-wide queries
    /// (the global `class_name` registry) take this as their salsa-tracked input.
    fn source_root(&self) -> Option<SourceRoot>;
    /// The project's `project.godot` config, or `None` in single-file mode. The autoload registry
    /// (M4) takes this as its salsa-tracked input.
    fn project_config(&self) -> Option<ProjectConfig>;
    /// The host-level warning-strictness override (default [`WarningOverride::None`]). A plain
    /// field, NOT a salsa input — read only by the downstream gate, so it never re-runs inference.
    fn warning_override(&self) -> WarningOverride;
}

/// The VFS leaf: one file's UTF-8 text, as a salsa input, plus its [`FileId`] (so a query
/// holding only a `FileText` can recover the id for cross-file resolution) and its `res://`
/// path (so `preload`/`extends "res://…"` resolve to the declaring file — M3).
///
/// `res_path` is a **separate salsa input field** from `text`: salsa tracks input fields
/// individually (per-field `revisions`/`durabilities` — verified against salsa 0.27.1
/// `input.rs`), so a query reading only `res_path` (the `res_path_registry`) *backdates* across
/// a `text` keystroke — exactly the firewall that protects `file_class_name`. It is held at
/// `MEDIUM` durability (set on file add, stable across edits); `text` stays `LOW`.
#[salsa::input(debug)]
pub struct FileText {
    /// The file's full text (interned `Arc<str>`; the getter returns `&Arc<str>`).
    #[returns(ref)]
    pub text: Arc<str>,
    /// The opaque file id this text belongs to.
    pub file_id: FileId,
    /// The file's project-relative `res://` path, if the loader supplied one (`None` in
    /// single-file mode / tests — then `preload`/`extends "res://…"` resolve to the seam).
    pub res_path: Option<smol_str::SmolStr>,
}

/// The project's file set — a salsa input so project-wide queries (the global `class_name`
/// registry, M1) iterate the files incrementally. It changes only when a file is **added or
/// removed**, never on a body edit, and is held at MEDIUM durability — so a keystroke (a `LOW`
/// change) never invalidates project-wide derived data.
#[salsa::input]
pub struct SourceRoot {
    /// Every file currently in the project, ordered by `FileId` for determinism.
    #[returns(ref)]
    pub files: Vec<FileText>,
    /// The loader's assertion that this file set is the **whole project** (every `.gd` under the
    /// project root was fed in). Default `false`. Absence-based diagnostics (`UNDEFINED_FUNCTION`
    /// / `UNDEFINED_IDENTIFIER`) key on this: proving a name is defined *nowhere* requires seeing
    /// *everywhere*, and neither `source_root().is_some()` (true after one lone file) nor
    /// `project_config().is_some()` (a single-file CLI run still discovers `project.godot`) can
    /// establish that — only the loader knows whether it walked the whole root.
    pub complete: bool,
}

/// The project's `project.godot`, injected as raw text — the wasm-clean core never reads the
/// filesystem, so the loader pushes the bytes exactly like a `.gd` file. The autoload index is a
/// tracked query that parses this text (M4). Held at `MEDIUM` durability (project structure,
/// stable across `.gd` keystrokes), so a body edit (LOW) never invalidates the autoload registry.
#[salsa::input]
pub struct ProjectConfig {
    /// The full `project.godot` text.
    #[returns(ref)]
    pub project_godot_text: Arc<str>,
}

/// A generation counter that makes the otherwise-untracked runtime engine model **invalidate**
/// correctly. The engine model is a leaked `&'static` side handle (not a salsa input), so a query
/// memoized while it was still absent (`engine() == None`, on `wasm32` before `set_engine_api`)
/// would otherwise return that stale empty result forever. Every `engine()` read records a
/// dependency on this input; `set_engine_api` bumps it, recomputing those queries. The *value* is
/// irrelevant — only that setting it advances the revision. Used on `wasm32` only (native has the
/// bundled model from the start, so it never changes — no generation tracking, no overhead).
#[salsa::input]
pub struct EngineGeneration {
    /// An opaque counter (only its revision matters).
    pub generation: u32,
}

/// The `FileId → FileText` side table. `Arc`-backed so a cheap clone shares the same map —
/// needed to mutate an input (`&mut dyn Db`) without simultaneously borrowing `self.files`.
#[derive(Debug, Default, Clone)]
pub struct Files {
    inner: Arc<DashMap<FileId, FileText, FxBuildHasher>>,
}

impl Files {
    /// The input for `file`, if set.
    #[must_use]
    pub fn file_text(&self, file: FileId) -> Option<FileText> {
        self.inner.get(&file).map(|r| *r)
    }

    /// Create or update `file`'s text input at `durability`. Creating uses `&db`; updating an
    /// existing input bumps the revision (`&mut db`), which is what cancels live read handles.
    pub fn set_file_text(&self, db: &mut dyn Db, file: FileId, text: &str, durability: Durability) {
        match self.inner.entry(file) {
            Entry::Occupied(occ) => {
                occ.get()
                    .set_text(db)
                    .with_durability(durability)
                    .to(Arc::from(text));
            }
            Entry::Vacant(vac) => {
                let ft = FileText::builder(Arc::from(text), file, None)
                    .durability(durability)
                    .new(db);
                vac.insert(ft);
            }
        }
    }

    /// Set `file`'s `res://` path at `MEDIUM` durability (stable project structure, like the
    /// source root). No-op if the file is unknown or the path is unchanged: salsa does **not**
    /// value-backdate an input setter (it bumps the field revision on *every* call, even for an
    /// identical value — verified against salsa 0.27.1 `input.rs:set_field`), so a redundant set
    /// would needlessly invalidate the `res_path_registry`. The guard keeps a re-`apply_change`
    /// of an already-known path free.
    pub fn set_file_path(&self, db: &mut dyn Db, file: FileId, path: &str) {
        let Some(ft) = self.inner.get(&file).map(|r| *r) else {
            return;
        };
        if ft.res_path(&*db).as_deref() == Some(path) {
            return;
        }
        ft.set_res_path(db)
            .with_durability(Durability::MEDIUM)
            .to(Some(smol_str::SmolStr::new(path)));
    }

    /// Drop `file` from the side table (its salsa input lingers, unreferenced, until GC).
    pub fn remove(&self, file: FileId) {
        self.inner.remove(&file);
    }

    /// Every file, ordered by `FileId` — the deterministic input to project-wide queries.
    fn all(&self) -> Vec<FileText> {
        let mut v: Vec<(FileId, FileText)> =
            self.inner.iter().map(|r| (*r.key(), *r.value())).collect();
        v.sort_by_key(|(id, _)| *id);
        v.into_iter().map(|(_, ft)| ft).collect()
    }
}

/// Parse a file to its lossless CST. Memoized; re-parses only when the file text changes.
#[salsa::tracked]
pub fn parse(db: &dyn Db, file: FileText) -> Parse {
    gdscript_syntax::parse(file.text(db))
}

/// The concrete analyzer database — a salsa `Storage` plus the [`Files`] side table.
#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
    files: Files,
    /// The project file-set input (lazily created on the first file change). Held outside salsa
    /// as a handle so `apply_change` can update it.
    root: Option<SourceRoot>,
    /// The `project.godot` config input (lazily created on the first config push). Held outside
    /// salsa as a handle so `apply_change` can update it (M4 autoloads).
    config: Option<ProjectConfig>,
    /// A runtime-injected engine model. `None` falls back to the bundled blob on native and to "no
    /// engine model" on `wasm32` (where nothing is embedded). The wasm binding fetches the blob and
    /// installs it here via [`RootDatabase::set_engine_api`] (Playbook §4.4). Held outside salsa (a
    /// process-lifetime `&'static`, leaked once).
    engine: Option<&'static EngineApi>,
    /// The host-level warning-strictness override (CLI `--strict`/`--engine-defaults`). A plain
    /// field — read only by the non-tracked `type_diagnostics`, never a salsa input.
    warning_override: WarningOverride,
    /// `wasm32`-only: the [`EngineGeneration`] input that makes a *later* `set_engine_api` invalidate
    /// queries memoized while the model was still absent (so the order "query, then load the engine"
    /// is correct, not just "load, then query"). Lazily created on the first structural change.
    #[cfg(target_arch = "wasm32")]
    engine_gen: Option<EngineGeneration>,
}

// `salsa::Storage` is not `Debug`, but the public `AnalysisHost`/`Analysis` that will own a
// `RootDatabase` must stay `Debug` (frozen API); hand-impl an opaque one.
impl std::fmt::Debug for RootDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootDatabase").finish_non_exhaustive()
    }
}

impl RootDatabase {
    /// Create/update `file`'s text input (the single input-mutation primitive `apply_change`
    /// drives). Clones the `Arc`-backed [`Files`] handle first so `self` is free to pass as the
    /// `&mut dyn Db` the salsa setter needs.
    pub fn set_file_text(&mut self, file: FileId, text: &str, durability: Durability) {
        let files = self.files.clone();
        files.set_file_text(self, file, text, durability);
    }

    /// Set `file`'s `res://` path (the loader supplies it on add; M3 `preload`/`extends` resolve
    /// through it). Guarded against no-op re-sets — see [`Files::set_file_path`].
    pub fn set_file_path(&mut self, file: FileId, path: &str) {
        let files = self.files.clone();
        files.set_file_path(self, file, path);
    }

    /// Remove `file`'s entry from the side table.
    pub fn remove_file(&mut self, file: FileId) {
        self.files.remove(file);
    }

    /// Set the host-level warning-strictness override (a CLI `--strict`/`--engine-defaults`
    /// policy). A plain field — changing it does not touch salsa, so it never re-runs inference;
    /// `type_diagnostics` re-reads it on the next snapshot.
    pub fn set_warning_override(&mut self, ov: WarningOverride) {
        self.warning_override = ov;
    }

    /// Set the project's `project.godot` text (the loader supplies it on project open / when it
    /// changes — M4 autoloads). No-op if unchanged: salsa bumps an input field's revision on
    /// every set even for an identical value, so a redundant push would needlessly invalidate the
    /// autoload registry. Held at `MEDIUM` durability, so a `.gd` keystroke never touches it.
    pub fn set_project_config(&mut self, text: &str) {
        if let Some(cfg) = self.config {
            if cfg.project_godot_text(self).as_ref() == text {
                return;
            }
            cfg.set_project_godot_text(self)
                .with_durability(Durability::MEDIUM)
                .to(Arc::from(text));
        } else {
            self.config = Some(
                ProjectConfig::builder(Arc::from(text))
                    .durability(Durability::MEDIUM)
                    .new(self),
            );
        }
    }

    /// Install a runtime-loaded engine model (the wasm path: a `fetch`ed `extension_api` blob
    /// decoded via [`EngineApi::from_bytes`]). Leaked to `&'static` (one per session, process
    /// lifetime). **Load-once before any query** — the engine model is not a salsa input, so a later
    /// set would not invalidate cached reads; first-wins (a redundant install is ignored, so the
    /// leak happens at most once). Native builds normally never call this (they fall back to the
    /// bundled blob); it is the seam the wasm/wasip1 binding uses.
    pub fn set_engine_api(&mut self, api: EngineApi) {
        if self.engine.is_none() {
            self.engine = Some(Box::leak(Box::new(api)));
            // wasm: advance the generation so any query memoized while the model was absent (the
            // "query before load" order) recomputes. Native never reaches here through the bindings,
            // and its bundled model is present from the start, so it needs no generation tracking.
            #[cfg(target_arch = "wasm32")]
            self.bump_engine_generation();
        }
    }

    /// wasm-only: create-or-advance the [`EngineGeneration`] input (see its docs). Creating it the
    /// first time is harmless; advancing it invalidates every query that read `engine()`.
    #[cfg(target_arch = "wasm32")]
    fn bump_engine_generation(&mut self) {
        if let Some(eg) = self.engine_gen {
            let next = eg.generation(self).wrapping_add(1);
            eg.set_generation(self)
                .with_durability(Durability::MEDIUM)
                .to(next);
        } else {
            self.engine_gen = Some(
                EngineGeneration::builder(0)
                    .durability(Durability::MEDIUM)
                    .new(self),
            );
        }
    }

    /// Rebuild the project file-set input from the current side table. Call this from
    /// `apply_change` **only when a file was added or removed** — never on a body edit — so the
    /// MEDIUM-durability project input (and everything derived from it) stays stable across
    /// keystrokes.
    pub fn sync_source_root(&mut self) {
        // wasm: ensure the engine generation exists before the first query runs, so every query's
        // `engine()` read records a dependency on it — otherwise a `set_engine_api` afterwards could
        // not invalidate a query that ran before the input existed. (The first structural change
        // always precedes the first query, since the Session early-returns for unknown URIs.)
        #[cfg(target_arch = "wasm32")]
        if self.engine_gen.is_none() {
            self.engine_gen = Some(
                EngineGeneration::builder(0)
                    .durability(Durability::MEDIUM)
                    .new(self),
            );
        }
        let files = self.files.all();
        if let Some(root) = self.root {
            root.set_files(self)
                .with_durability(Durability::MEDIUM)
                .to(files);
        } else {
            // A fresh root starts INCOMPLETE — only the loader's explicit claim
            // (`set_workspace_complete`) flips it.
            let root = SourceRoot::builder(files, false)
                .durability(Durability::MEDIUM)
                .new(self);
            self.root = Some(root);
        }
    }

    /// Record the loader's claim that the current file set is the **whole project** (see
    /// [`SourceRoot::complete`]). No-op if unchanged (salsa bumps an input field's revision on
    /// every set, even for an identical value). Creates the (empty) root if none exists yet so
    /// the claim survives a set-before-first-file ordering.
    pub fn set_workspace_complete(&mut self, complete: bool) {
        if let Some(root) = self.root {
            if root.complete(self) != complete {
                root.set_complete(self)
                    .with_durability(Durability::MEDIUM)
                    .to(complete);
            }
        } else {
            let root = SourceRoot::builder(self.files.all(), complete)
                .durability(Durability::MEDIUM)
                .new(self);
            self.root = Some(root);
        }
    }
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn file_text(&self, file: FileId) -> Option<FileText> {
        self.files.file_text(file)
    }

    // A runtime-injected model wins; else native falls back to the bundled blob and wasm32 to
    // `None` (until the binding installs a fetched blob). clippy sees one target per build.
    #[allow(clippy::unnecessary_wraps)]
    fn engine(&self) -> Option<&'static EngineApi> {
        // wasm: record a dependency on the generation so a later `set_engine_api` invalidates this
        // read. (Native skips this entirely — the bundled model is constant, so zero overhead.)
        #[cfg(target_arch = "wasm32")]
        if let Some(eg) = self.engine_gen {
            let _ = eg.generation(self);
        }
        if let Some(api) = self.engine {
            return Some(api);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Some(gdscript_api::bundled())
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    fn source_root(&self) -> Option<SourceRoot> {
        self.root
    }

    fn project_config(&self) -> Option<ProjectConfig> {
        self.config
    }

    fn warning_override(&self) -> WarningOverride {
        self.warning_override
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_returns_a_cst() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "func f():\n\tpass\n", Durability::LOW);
        let ft = db.file_text(FileId(0)).unwrap();
        let p = parse(&db, ft);
        assert!(p.errors().is_empty());
        // Re-querying the same input returns the memoized value (no re-parse).
        assert_eq!(parse(&db, ft).debug_tree(), p.debug_tree());
    }

    #[test]
    fn set_get_remove_round_trips() {
        let mut db = RootDatabase::default();
        let id = FileId(7);
        db.set_file_text(id, "var x = 1\n", Durability::LOW);
        assert_eq!(db.file_text(id).unwrap().text(&db).as_ref(), "var x = 1\n");
        // Update in place.
        db.set_file_text(id, "var y = 2\n", Durability::LOW);
        assert_eq!(db.file_text(id).unwrap().text(&db).as_ref(), "var y = 2\n");
        // Remove.
        db.remove_file(id);
        assert!(db.file_text(id).is_none());
    }

    #[test]
    fn res_path_round_trips_and_guards_no_op_sets() {
        let mut db = RootDatabase::default();
        let id = FileId(3);
        // No path until the loader sets one.
        db.set_file_text(id, "class_name A\n", Durability::LOW);
        assert_eq!(db.file_text(id).unwrap().res_path(&db), None);
        // Set, then read back.
        db.set_file_path(id, "res://a.gd");
        assert_eq!(
            db.file_text(id).unwrap().res_path(&db).as_deref(),
            Some("res://a.gd")
        );
        // A re-set of the SAME path is a guarded no-op (does not panic / regress); a real rename
        // updates it.
        db.set_file_path(id, "res://a.gd");
        db.set_file_path(id, "res://b.gd");
        assert_eq!(
            db.file_text(id).unwrap().res_path(&db).as_deref(),
            Some("res://b.gd")
        );
        // Setting a path for an unknown file is a no-op (no panic).
        db.set_file_path(FileId(999), "res://ghost.gd");
        assert!(db.file_text(FileId(999)).is_none());
    }
}
