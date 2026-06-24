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

mod features;
mod semantic;

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
        for (id, text) in change.files {
            match text {
                Some(t) => self.db.set_file_text(id, &t, Durability::LOW),
                None => self.db.remove_file(id),
            }
        }
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
                    semantic::member_completions(&self.db, ft, pos.offset)
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

    /// Go-to-definition (Phase 2+).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    #[allow(clippy::unused_self)]
    pub fn goto_definition(
        &self,
        _pos: FilePosition,
    ) -> Cancellable<Vec<gdscript_base::TextRange>> {
        catch(Vec::new)
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
    fn removing_a_file_clears_it() {
        let (mut host, file) = host_with("var x = 1\n");
        let mut change = Change::new();
        change.remove_file(file);
        host.apply_change(change);
        let analysis = host.analysis();
        assert!(analysis.document_symbols(file).unwrap().is_empty());
    }
}
