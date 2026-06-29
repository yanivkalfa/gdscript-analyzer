//! `gdscript-ide` — the public, engine-/protocol-neutral analysis API.
//!
//! Modeled on rust-analyzer's `ide::AnalysisHost` / `ide::Analysis`
//! (`plans/01-ARCHITECTURE.md` §2). [`AnalysisHost`] is the single mutable owner of the
//! input world; [`Analysis`] is a cheap, cloneable, `Send` snapshot whose queries take
//! byte offsets and return plain `serde` result structs from `gdscript-base` — never
//! `lsp-types`. Each client (LSP server, the guitkx adapter, the CLI, the WASM
//! playground) maps these POD results to its own protocol.
//!
//! Phase 3 (M0) swaps the engine behind these types from a plain VFS map to a **salsa**
//! query graph in [`gdscript_db`]: the input world is now `FileText` salsa inputs, mutated
//! through `apply_change`; [`Analysis`] is a cloned database handle (salsa handles are
//! `Clone + Send`, replacing the old `Arc<map>` snapshot). Cancellation is now *real* —
//! a concurrent `apply_change` cancels in-flight reads on outstanding handles, which unwind
//! into `Err(Cancelled)` at the query boundary (see [`catch`]). The public API shape is
//! unchanged. The crate stays `wasm32`-safe (CI guards this).
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use gdscript_base::{
    Cancellable, CodeAction, CompletionItem, Diagnostic, DocumentSymbol, FileId, FilePosition,
    FoldRange, HoverResult, InlayHint, SignatureHelp,
};
use gdscript_db::{Db, RootDatabase};
use salsa::Durability;

/// Re-exported so clients can set the warning-strictness override without depending on
/// `gdscript-db` directly. See [`AnalysisHost::set_warning_override`].
pub use gdscript_db::WarningOverride;

mod features;
mod navigation;
mod semantic;
mod semantic_tokens;

/// Run a read query, turning a salsa cancellation (a concurrent `apply_change` invalidated the
/// snapshot) into `Err(Cancelled)`. The closure is `AssertUnwindSafe` because the database
/// handle it borrows is shared, immutable for the duration of the read, and salsa's unwind is
/// panic-safe by design.
fn catch<T>(f: impl FnOnce() -> T) -> Cancellable<T> {
    salsa::Cancelled::catch(std::panic::AssertUnwindSafe(f)).map_err(|_| gdscript_base::Cancelled)
}

/// The single mutable owner of analysis state — one per project/workspace.
///
/// The input world is a virtual file system (`FileId` → UTF-8 text) held as salsa inputs; the
/// host never reads paths. Clients push text via [`AnalysisHost::apply_change`].
#[derive(Debug, Clone, Default)]
pub struct AnalysisHost {
    db: RootDatabase,
}

/// A batch of input changes. `None` text removes the file.
#[derive(Debug, Default)]
pub struct Change {
    /// Files to add/replace (`Some`) or remove (`None`).
    pub files: Vec<(FileId, Option<Arc<str>>)>,
    /// Each file's `res://` path (loader-supplied; M3 `preload`/`extends "res://…"` resolution).
    /// Supply it when a file is **added**; it is stable across edits, so a keystroke change must
    /// omit it (salsa bumps an input field's revision on *every* set, even an identical value, so
    /// re-sending a path each edit would needlessly invalidate the `res_path_registry`).
    pub paths: Vec<(FileId, String)>,
    /// The project's `project.godot` text (loader-supplied; M4 `[autoload]` resolution). Set once
    /// on project open / when it changes; omit on `.gd` keystrokes.
    pub project_config: Option<Arc<str>>,
}

impl Change {
    /// An empty change set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a file add/replace.
    pub fn change_file(&mut self, file: FileId, text: impl Into<Arc<str>>) {
        self.files.push((file, Some(text.into())));
    }

    /// Queue a file removal.
    pub fn remove_file(&mut self, file: FileId) {
        self.files.push((file, None));
    }

    /// Record a file's `res://` path (the project-relative resource path the loader assigns). Set
    /// it once, when the file is first added; omit it on subsequent edits.
    pub fn set_file_path(&mut self, file: FileId, path: impl Into<String>) {
        self.paths.push((file, path.into()));
    }

    /// Record the project's `project.godot` text (M4 `[autoload]` resolution). Set on project open
    /// / when it changes; omit on `.gd` keystrokes.
    pub fn set_project_config(&mut self, text: impl Into<Arc<str>>) {
        self.project_config = Some(text.into());
    }
}

impl AnalysisHost {
    /// A new, empty host.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a batch of input changes. The **only** mutation entry point — each `set`/`remove`
    /// bumps the salsa revision (and cancels any in-flight reads on outstanding [`Analysis`]
    /// handles). Edited files are `LOW` durability (they change every keystroke).
    pub fn apply_change(&mut self, change: Change) {
        let mut structure_changed = false;
        for (id, text) in change.files {
            if let Some(t) = text {
                // A file the project hasn't seen before changes the file *set*.
                structure_changed |= self.db.file_text(id).is_none();
                self.db.set_file_text(id, &t, Durability::LOW);
            } else {
                structure_changed |= self.db.file_text(id).is_some();
                self.db.remove_file(id);
            }
        }
        // Apply `res://` paths (loader-supplied, on add). `set_file_path` no-ops when the path is
        // unchanged, so this never invalidates the `res_path_registry` on a redundant set; the
        // FileText must already exist, so it runs after the text loop above.
        for (id, path) in change.paths {
            self.db.set_file_path(id, &path);
        }
        // The `project.godot` config (M4 autoloads) — its own MEDIUM input, guarded against no-op
        // re-sets, so re-opening a project doesn't invalidate the autoload registry.
        if let Some(text) = change.project_config {
            self.db.set_project_config(&text);
        }
        // Rebuild the project file-set input ONLY on add/remove — never on a body edit — so the
        // MEDIUM-durability registry stays firewalled against keystrokes.
        if structure_changed {
            self.db.sync_source_root();
        }
    }

    /// Install a runtime-fetched engine model — the **wasm path** (an `extension_api` blob the host
    /// `fetch`ed and brotli-decoded, decoded here via `EngineApi::from_bytes`). Native builds use the
    /// bundled model and normally never call this. Returns `false` (rather than panicking) if the
    /// bytes fail to decode, leaving the model unset. First install wins (load-once); installing it
    /// **after** queries have already run correctly recomputes them — the wasm engine-generation
    /// input invalidates the affected reads, so loading the blob async (after opening a document) is
    /// safe, not just loading it first.
    pub fn set_engine_api(&mut self, bytes: &[u8]) -> bool {
        match gdscript_api::EngineApi::from_bytes(bytes) {
            Ok(api) => {
                self.db.set_engine_api(api);
                true
            }
            Err(_) => false,
        }
    }

    /// Force a warning-strictness baseline regardless of `project.godot` presence (the CLI
    /// `--strict` / `--engine-defaults` knob; an LSP could set it per session). A plain `Db` field,
    /// not a salsa input — changing it never re-runs inference, only the downstream gate.
    pub fn set_warning_override(&mut self, ov: gdscript_db::WarningOverride) {
        self.db.set_warning_override(ov);
    }

    /// A cheap, cloneable, `Send` snapshot for read queries (a cloned salsa database handle).
    #[must_use]
    pub fn analysis(&self) -> Analysis {
        Analysis {
            db: self.db.clone(),
        }
    }
}

/// An immutable snapshot of the world — a cloned salsa handle. Every query is [`Cancellable`]:
/// a concurrent `apply_change` cancels in-flight reads, which the client re-issues against the
/// fresh snapshot.
#[derive(Debug, Clone)]
pub struct Analysis {
    db: RootDatabase,
}

impl Analysis {
    // ---- Tier-0 features: real data ----

    /// A pretty-printed dump of the syntax tree (debugging / playground).
    ///
    /// # Errors
    /// `Err(Cancelled)` if a concurrent `apply_change` invalidated this snapshot.
    pub fn syntax_tree(&self, file: FileId) -> Cancellable<Option<String>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| gdscript_db::parse(&self.db, ft).debug_tree())
        })
    }

    /// Parse-error diagnostics ∪ the Phase-2 §5 type diagnostics.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn diagnostics(&self, file: FileId) -> Cancellable<Vec<Diagnostic>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| {
                    let mut diags = features::diagnostics(&self.db, ft);
                    diags.extend(semantic::type_diagnostics(&self.db, ft));
                    diags
                })
                .unwrap_or_default()
        })
    }

    /// Format `file`'s source, returning the tidied text — or `None` if the file is unknown.
    /// Safe by construction: it normalizes whitespace + indentation and never changes meaning,
    /// falling back to the original on anything it can't safely reformat (see [`gdscript_fmt`]).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn format(&self, file: FileId) -> Cancellable<Option<String>> {
        catch(|| {
            self.db.file_text(file).map(|ft| {
                gdscript_fmt::format(ft.text(&self.db), &gdscript_fmt::FmtConfig::default())
            })
        })
    }

    /// Format only the lines overlapping the byte range `[start, end)` (editor "format selection").
    /// Returns the byte range to replace and its replacement, or `None` if the selection's lines do
    /// not change (or the file is unknown).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn format_range(
        &self,
        file: FileId,
        start: u32,
        end: u32,
    ) -> Cancellable<Option<(u32, u32, String)>> {
        catch(|| {
            self.db.file_text(file).and_then(|ft| {
                let sel = (start as usize)..(end as usize);
                gdscript_fmt::format_range(
                    ft.text(&self.db),
                    &gdscript_fmt::FmtConfig::default(),
                    sel,
                )
                .map(|e| {
                    (
                        u32::try_from(e.range.start).unwrap_or(u32::MAX),
                        u32::try_from(e.range.end).unwrap_or(u32::MAX),
                        e.new_text,
                    )
                })
            })
        })
    }

    /// The document outline (classes, funcs, vars, consts, enums, signals, members).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn document_symbols(&self, file: FileId) -> Cancellable<Vec<DocumentSymbol>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| features::document_symbols(&self.db, ft))
                .unwrap_or_default()
        })
    }

    /// Semantic-highlighting tokens: each meaningful token classified by its contextual role
    /// (declarations, types, parameters, members, calls, literals, comments) — richer than a
    /// grammar. In source order.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn semantic_tokens(&self, file: FileId) -> Cancellable<Vec<gdscript_base::SemanticToken>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| semantic_tokens::semantic_tokens(&self.db, ft))
                .unwrap_or_default()
        })
    }

    /// Foldable ranges (blocks, `#region` pairs, multi-line brackets).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn folding_ranges(&self, file: FileId) -> Cancellable<Vec<FoldRange>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| features::folding_ranges(&self.db, ft))
                .unwrap_or_default()
        })
    }

    /// Completions. After `receiver.` it offers the inferred member set; otherwise (or when
    /// the receiver is `Variant`/`Unknown`) it falls back to the Tier-0 by-name completion
    /// (keywords, annotations after `@`, document-local symbols) so it never regresses.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn completions(&self, pos: FilePosition) -> Cancellable<Vec<CompletionItem>> {
        catch(|| {
            self.db
                .file_text(pos.file)
                .map(|ft| {
                    semantic::node_path_completions(&self.db, ft, pos.offset)
                        .or_else(|| semantic::member_completions(&self.db, ft, pos.offset))
                        .unwrap_or_else(|| features::completions(&self.db, ft, pos.offset))
                })
                .unwrap_or_default()
        })
    }

    /// Hover: the inferred type of the expression / binding under the cursor (`Unknown`
    /// elided). `None` when there is nothing typed there.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn hover(&self, pos: FilePosition) -> Cancellable<Option<HoverResult>> {
        catch(|| {
            self.db
                .file_text(pos.file)
                .and_then(|ft| semantic::hover(&self.db, ft, pos.offset))
        })
    }

    /// Inlay `: T` hints on `:=` declarations + unannotated params / `for`-vars (suppressed
    /// when the type is `Variant`/`Unknown`).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn inlay_hints(&self, file: FileId) -> Cancellable<Vec<InlayHint>> {
        catch(|| {
            self.db
                .file_text(file)
                .map(|ft| semantic::inlay_hints(&self.db, ft))
                .unwrap_or_default()
        })
    }

    /// Signature help at a call site (active parameter by top-level comma count).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn signature_help(&self, pos: FilePosition) -> Cancellable<Option<SignatureHelp>> {
        catch(|| {
            self.db
                .file_text(pos.file)
                .and_then(|ft| semantic::signature_help(&self.db, ft, pos.offset))
        })
    }

    /// Code actions at a position (currently "add type annotation").
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn code_actions(&self, pos: FilePosition) -> Cancellable<Vec<CodeAction>> {
        catch(|| {
            self.db
                .file_text(pos.file)
                .map(|ft| semantic::code_actions(&self.db, ft, pos.offset))
                .unwrap_or_default()
        })
    }

    /// Go-to-definition: the declaration target(s) of the symbol under the cursor (cross-file).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn goto_definition(&self, pos: FilePosition) -> Cancellable<Vec<gdscript_base::NavTarget>> {
        catch(|| navigation::goto_definition(&self.db, pos))
    }

    /// Find every reference to the symbol under the cursor, project-wide (incl. its declaration).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn find_references(&self, pos: FilePosition) -> Cancellable<Vec<gdscript_base::Reference>> {
        catch(|| navigation::find_references(&self.db, pos))
    }

    /// Rename the symbol under the cursor to `new_name` — a cross-file edit, or a refusal
    /// ([`RenameError`](gdscript_base::RenameError)); never a partial edit.
    ///
    /// # Errors
    /// `Err(Cancelled)` if a concurrent `apply_change` invalidated this snapshot. The rename's own
    /// refusal is the `Result` *inside* the `Cancellable`.
    pub fn rename(
        &self,
        pos: FilePosition,
        new_name: &str,
    ) -> Cancellable<Result<gdscript_base::SourceChange, gdscript_base::RenameError>> {
        catch(|| navigation::rename(&self.db, pos, new_name))
    }

    /// Project-wide symbols matching `query` (fuzzy-ranked class names + members).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn workspace_symbols(&self, query: &str) -> Cancellable<Vec<gdscript_base::NavTarget>> {
        catch(|| navigation::workspace_symbols(&self.db, query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with(src: &str) -> (AnalysisHost, FileId) {
        let mut host = AnalysisHost::new();
        let file = FileId(0);
        let mut change = Change::new();
        change.change_file(file, src);
        host.apply_change(change);
        (host, file)
    }

    #[test]
    fn snapshot_reads_applied_files() {
        let (host, file) = host_with("func f():\n\tpass\n");
        let analysis = host.analysis();
        let symbols = analysis.document_symbols(file).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "f");
    }

    #[test]
    fn preload_resolves_cross_file_through_the_public_api() {
        // The real `guitkx.gd` pattern, end-to-end through `apply_change` + `set_file_path`:
        // `const M = preload("res://…")` then `M.new().method()`.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "class_name Markup\nfunc parse() -> int:\n\treturn 1\n",
        );
        change.set_file_path(FileId(0), "res://markup.gd");
        change.change_file(
            FileId(1),
            "const M = preload(\"res://markup.gd\")\nfunc go():\n\tvar n := M.new().parse()\n\treturn n\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        // Valid code → no diagnostics.
        assert!(analysis.diagnostics(FileId(1)).unwrap().is_empty());
        // The cross-file preload resolved, so `n` is typed `int`; an inlay hint proves it (an
        // *unresolved* preload would leave `n` on the seam, suppressing the hint).
        let hints = analysis.inlay_hints(FileId(1)).unwrap();
        assert!(
            hints.iter().any(|h| h.label.contains("int")),
            "expected an `: int` inlay on the preload-resolved binding, got {hints:?}",
        );
    }

    #[test]
    fn autoload_resolves_cross_file_through_the_public_api() {
        // End-to-end through `apply_change` + `set_project_config`: a `*`-singleton autoload
        // script (no class_name — resolved by path) used by its bare name.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(FileId(0), "func volume() -> int:\n\treturn 50\n");
        change.set_file_path(FileId(0), "res://audio.gd");
        change.change_file(
            FileId(1),
            "func go():\n\tvar v := Audio.volume()\n\treturn v\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        change.set_project_config("[autoload]\nAudio=\"*res://audio.gd\"\n");
        host.apply_change(change);
        let analysis = host.analysis();

        assert!(analysis.diagnostics(FileId(1)).unwrap().is_empty());
        // `Audio.volume()` resolved cross-file via the autoload singleton → `v : int` inlay.
        let hints = analysis.inlay_hints(FileId(1)).unwrap();
        assert!(
            hints.iter().any(|h| h.label.contains("int")),
            "expected an `: int` inlay on the autoload-resolved binding, got {hints:?}",
        );
    }

    #[test]
    fn multi_scene_node_path_unions_to_the_common_base() {
        // main.gd attaches to a.tscn (`$Btn`: HBoxContainer) AND b.tscn (`$Btn`: VBoxContainer). The
        // path unions to the common base BoxContainer (both extend it) — not the first scene's type.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Btn\" type=\"HBoxContainer\" parent=\".\"]\n",
        );
        change.set_file_path(FileId(0), "res://a.tscn");
        change.change_file(
            FileId(2),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Btn\" type=\"VBoxContainer\" parent=\".\"]\n",
        );
        change.set_file_path(FileId(2), "res://b.tscn");
        change.change_file(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar b := $Btn\n\tb.queue_free()\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        let hints = analysis.inlay_hints(FileId(1)).unwrap();
        assert!(
            hints.iter().any(|h| h.label.contains("BoxContainer")),
            "expected the common base `: BoxContainer` of HBox/VBoxContainer, got {hints:?}",
        );
    }

    #[test]
    fn non_singleton_autoload_resolves_via_root_path() {
        // A non-`*` autoload is loaded-but-not-global: unreachable by bare name, but reachable via
        // the absolute `get_node("/root/Name")` path. `.volume()` must resolve through its script.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(FileId(0), "func volume() -> int:\n\treturn 50\n");
        change.set_file_path(FileId(0), "res://audio.gd");
        change.change_file(
            FileId(1),
            "func go():\n\tvar v := get_node(\"/root/Audio\").volume()\n\treturn v\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        // No leading `*` → loaded-but-not-global. Bare `Audio` would NOT resolve; `/root/Audio` does.
        change.set_project_config("[autoload]\nAudio=\"res://audio.gd\"\n");
        host.apply_change(change);
        let analysis = host.analysis();

        assert!(analysis.diagnostics(FileId(1)).unwrap().is_empty());
        let hints = analysis.inlay_hints(FileId(1)).unwrap();
        assert!(
            hints.iter().any(|h| h.label.contains("int")),
            "expected an `: int` inlay on the /root/-autoload-resolved binding, got {hints:?}",
        );
    }

    #[test]
    fn scene_node_path_typing_through_the_public_api() {
        // The Phase-4 killer feature end-to-end: a `.tscn` injected via `apply_change` + a script it
        // attaches → `$Btn` types as `Button`, surfaced as an `: Button` inlay (zero annotations).
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Btn\" type=\"Button\" parent=\".\"]\n",
        );
        change.set_file_path(FileId(0), "res://main.tscn");
        change.change_file(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar b := $Btn\n\tb.show()\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        assert!(analysis.diagnostics(FileId(1)).unwrap().is_empty());
        let hints = analysis.inlay_hints(FileId(1)).unwrap();
        assert!(
            hints.iter().any(|h| h.label.contains("Button")),
            "expected a `: Button` inlay on `var b := $Btn`, got {hints:?}",
        );
    }

    #[test]
    fn node_path_completion_offers_scene_children() {
        // `$Panel/` offers Panel's children (typed by their `type=`); `$` offers the attach node's.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Panel\" type=\"Panel\" parent=\".\"]\n\
             [node name=\"Ok\" type=\"Button\" parent=\"Panel\"]\n\
             [node name=\"Cancel\" type=\"Button\" parent=\"Panel\"]\n",
        );
        change.set_file_path(FileId(0), "res://main.tscn");
        let gd = "extends Control\nfunc _ready():\n\tvar b := $Panel/\n";
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        let offset = u32::try_from(gd.find("$Panel/").unwrap() + "$Panel/".len()).unwrap();
        let items = analysis
            .completions(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Ok") && labels.contains(&"Cancel"),
            "{labels:?}"
        );
        // node completions are typed by their `type=` and don't leak keywords/locals here.
        assert!(
            items
                .iter()
                .find(|i| i.label == "Ok")
                .is_some_and(|i| i.detail.as_deref() == Some("Button")),
            "{items:?}",
        );
        assert!(
            !labels.contains(&"func"),
            "should be node-path, not keyword, completion"
        );
    }

    #[test]
    fn node_path_completion_does_not_hijack_inside_a_string_literal() {
        // A `$child/` that appears INSIDE a string literal must NOT trigger node-path completion
        // (the byte scan has no lexer awareness; a `String` token at the cursor suppresses it).
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Panel\" type=\"Panel\" parent=\".\"]\n\
             [node name=\"Ok\" type=\"Button\" parent=\"Panel\"]\n",
        );
        change.set_file_path(FileId(0), "res://main.tscn");
        let gd = "extends Control\nfunc _ready():\n\tvar s := \"$Panel/\"\n";
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        // cursor right after the `/`, INSIDE the string literal.
        let offset = u32::try_from(gd.find("$Panel/").unwrap() + "$Panel/".len()).unwrap();
        let items = analysis
            .completions(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap();
        assert!(
            !items.iter().any(|i| i.label == "Ok"),
            "node names must not leak into a string literal: {items:?}",
        );
    }

    #[test]
    fn unique_node_path_completion_offers_children() {
        // `%Box/` resolves the unique node `Box` scene-wide and offers its children, typed by `type=`.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let scene = "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Box\" type=\"Panel\" parent=\".\"]\n\
             unique_name_in_owner = true\n\
             [node name=\"Ok\" type=\"Button\" parent=\"Box\"]\n\
             [node name=\"Cancel\" type=\"Button\" parent=\"Box\"]\n";
        change.change_file(FileId(0), scene);
        change.set_file_path(FileId(0), "res://main.tscn");
        let gd = "extends Control\nfunc _ready():\n\tvar b := %Box/\n";
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();
        let offset = u32::try_from(gd.find("%Box/").unwrap() + "%Box/".len()).unwrap();
        let items = analysis
            .completions(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Ok") && labels.contains(&"Cancel"),
            "{labels:?}"
        );
        assert!(
            !labels.contains(&"func"),
            "node-path, not keyword completion"
        );
    }

    #[test]
    fn bare_percent_offers_all_unique_nodes() {
        // A bare `%` offers every unique node in the owning scene (scene-wide), not just children.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let scene = "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Box\" type=\"Panel\" parent=\".\"]\n\
             unique_name_in_owner = true\n\
             [node name=\"Hud\" type=\"Control\" parent=\".\"]\n\
             unique_name_in_owner = true\n";
        change.change_file(FileId(0), scene);
        change.set_file_path(FileId(0), "res://main.tscn");
        let gd = "extends Control\nfunc _ready():\n\tvar b := %\n";
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();
        let offset = u32::try_from(gd.find("%\n").unwrap() + 1).unwrap();
        let labels: Vec<_> = analysis
            .completions(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap()
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert!(
            labels.iter().any(|l| l == "Box") && labels.iter().any(|l| l == "Hud"),
            "{labels:?}"
        );
    }

    #[test]
    fn percent_modulo_is_not_hijacked_as_a_unique_path() {
        // `count % Box` is modulo, not a unique-node path — completion must stay by-name (the parsed
        // `%` token's parent is `BinExpr`, not `UniqueNodeExpr`).
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let scene = "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Box\" type=\"Panel\" parent=\".\"]\n\
             unique_name_in_owner = true\n";
        change.change_file(FileId(0), scene);
        change.set_file_path(FileId(0), "res://main.tscn");
        let gd = "extends Control\nfunc _ready():\n\tvar count := 10\n\tvar b := count %Box\n";
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();
        let offset = u32::try_from(gd.find("%Box").unwrap() + "%Box".len()).unwrap();
        let labels: Vec<_> = analysis
            .completions(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap()
            .into_iter()
            .map(|i| i.label)
            .collect();
        // By-name completion ran (keywords present), node-path did not hijack the modulo.
        assert!(
            labels.iter().any(|l| l == "func"),
            "expected by-name completion: {labels:?}"
        );
    }

    #[test]
    fn completion_is_scope_aware_for_locals_and_params() {
        // By-name completion must offer class members everywhere, but a parameter / local of one
        // function must NOT leak into a sibling function. The enclosing function is found by
        // indentation, so completing on a fresh (empty) indented line at the end of a body still
        // sees that body's own params/locals (the case the CST-range approach regressed).
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let gd = "var member_v := 0\nfunc a(pa):\n\tvar la := 1\n\t\nfunc b(pb):\n\tvar lb := 2\n";
        change.change_file(FileId(0), gd);
        change.set_file_path(FileId(0), "res://m.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        // Cursor on the empty indented line inside a() (right after the body's tab).
        let upto = "var member_v := 0\nfunc a(pa):\n\tvar la := 1\n\t";
        let offset = u32::try_from(gd.find(upto).unwrap() + upto.len()).unwrap();
        let items = analysis
            .completions(FilePosition {
                file: FileId(0),
                offset,
            })
            .unwrap();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        // Own param + own local + the class member + both func names are visible.
        assert!(labels.contains(&"pa"), "own param `pa`: {labels:?}");
        assert!(labels.contains(&"la"), "own local `la`: {labels:?}");
        assert!(labels.contains(&"member_v"), "class member: {labels:?}");
        assert!(
            labels.contains(&"a") && labels.contains(&"b"),
            "sibling func names: {labels:?}",
        );
        // b()'s param + local must NOT leak into a().
        assert!(!labels.contains(&"pb"), "leaked b's param: {labels:?}");
        assert!(!labels.contains(&"lb"), "leaked b's local: {labels:?}");
    }

    #[test]
    fn completion_at_class_level_offers_members_not_locals() {
        // At class level (no enclosing function) only members are offered — no function's locals.
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let gd = "var member_v := 0\nfunc a():\n\tvar la := 1\n\nm\n";
        change.change_file(FileId(0), gd);
        change.set_file_path(FileId(0), "res://m.gd");
        host.apply_change(change);
        let analysis = host.analysis();
        // Cursor after the top-level `m` (class level, indent 0).
        let offset = u32::try_from(gd.rfind('m').unwrap() + 1).unwrap();
        let items = analysis
            .completions(FilePosition {
                file: FileId(0),
                offset,
            })
            .unwrap();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"member_v") && labels.contains(&"a"),
            "{labels:?}"
        );
        assert!(
            !labels.contains(&"la"),
            "a()'s local must not leak to class level: {labels:?}"
        );
    }

    #[test]
    fn completion_offers_params_in_lambda_setter_and_inline_bodies() {
        // Regression (bug-hunt): the scope filter must offer a callable's own params inside its body
        // for ALL callable kinds, not just multi-line `func`s: a top-level named lambda, a `get`/`set`
        // accessor, and a one-line `func`. (The indentation-only scan missed these, hiding the param.)
        let cases = [
            // (source, the param that must be offered, a marker the cursor is placed right after)
            ("var f := func(px):\n\treturn px\n", "px", "return "),
            ("var x: int:\n\tset(sv):\n\t\t_x = sv\n", "sv", "_x = "),
            ("func foo(ia): return ia\n", "ia", "return "),
        ];
        for (gd, param, marker) in cases {
            let mut host = AnalysisHost::new();
            let mut change = Change::new();
            change.change_file(FileId(0), gd);
            change.set_file_path(FileId(0), "res://m.gd");
            host.apply_change(change);
            let analysis = host.analysis();
            let offset = u32::try_from(gd.find(marker).unwrap() + marker.len()).unwrap();
            let labels: Vec<_> = analysis
                .completions(FilePosition {
                    file: FileId(0),
                    offset,
                })
                .unwrap()
                .into_iter()
                .map(|i| i.label)
                .collect();
            assert!(
                labels.iter().any(|l| l == param),
                "param `{param}` should be offered inside its body for {gd:?}, got {labels:?}",
            );
        }
    }

    #[test]
    fn goto_definition_on_a_node_path_jumps_into_the_tscn() {
        // Cursor on `$Btn` → a NavTarget pointing at the `[node name="Btn" …]` line in the owning
        // `.tscn` (the inverse of M1 typing; navigation the engine LSP cannot provide).
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        let scene = "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Btn\" type=\"Button\" parent=\".\"]\n";
        let gd = "extends Control\nfunc _ready():\n\tvar b := $Btn\n";
        change.change_file(FileId(0), scene);
        change.set_file_path(FileId(0), "res://main.tscn");
        change.change_file(FileId(1), gd);
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();

        let offset = u32::try_from(gd.find("$Btn").unwrap() + 1).unwrap(); // on the `B`
        let targets = analysis
            .goto_definition(FilePosition {
                file: FileId(1),
                offset,
            })
            .unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file, FileId(0), "jumps into the .tscn");
        let focus =
            &scene[targets[0].focus_range.start as usize..targets[0].focus_range.end as usize];
        assert!(
            focus.contains("Btn"),
            "focus on the node name, got {focus:?}"
        );
    }

    #[test]
    fn find_refs_and_rename_cross_file_through_the_public_api() {
        let mut host = AnalysisHost::new();
        let mut change = Change::new();
        change.change_file(
            FileId(0),
            "class_name Widget\nfunc make() -> int:\n\treturn 1\n",
        );
        change.set_file_path(FileId(0), "res://widget.gd");
        change.change_file(
            FileId(1),
            "func f():\n\tvar w: Widget\n\tvar x := Widget.new()\n",
        );
        change.set_file_path(FileId(1), "res://main.gd");
        host.apply_change(change);
        let analysis = host.analysis();
        // The `class_name Widget` declaration name starts at offset 11 (`"class_name "` is 11).
        let at_decl = FilePosition {
            file: FileId(0),
            offset: 11,
        };
        // find-refs: declaration (f0) + annotation + `.new()` (f1) = 3.
        let refs = analysis.find_references(at_decl).unwrap();
        assert_eq!(refs.len(), 3, "{refs:?}");
        // rename → a cross-file SourceChange touching both files.
        let edit = analysis
            .rename(at_decl, "Gadget")
            .unwrap()
            .expect("rename ok");
        assert_eq!(edit.edits.len(), 2, "both files edited");
    }

    #[test]
    fn removing_a_file_clears_it() {
        let (mut host, file) = host_with("var x = 1\n");
        let mut change = Change::new();
        change.remove_file(file);
        host.apply_change(change);
        let analysis = host.analysis();
        assert!(analysis.document_symbols(file).unwrap().is_empty());
    }
}
