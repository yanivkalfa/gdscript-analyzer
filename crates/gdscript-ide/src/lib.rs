//! `gdscript-ide` — the public, engine-/protocol-neutral analysis API.
//!
//! Modeled on rust-analyzer's `ide::AnalysisHost` / `ide::Analysis`
//! (`plans/01-ARCHITECTURE.md` §2). [`AnalysisHost`] is the single mutable owner of the
//! input world; [`Analysis`] is a cheap, cloneable, `Send` snapshot whose queries take
//! byte offsets and return plain `serde` result structs from `gdscript-base` — never
//! `lsp-types`. Each client (LSP server, the guitkx adapter, the CLI, the WASM
//! playground) maps these POD results to its own protocol.
//!
//! Phase 1 (Tier 0) implements four features for real — parse diagnostics, document
//! symbols, folding ranges, and by-name completion — and stubs the rest. There is no
//! salsa yet: a plain VFS map and whole-file reparse on query. Every derived
//! computation is a pure `(text) -> value` function so the Phase-3 salsa swap is
//! localized. The crate is `wasm32`-safe (CI guards this).
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use rustc_hash::FxHashMap;

use gdscript_base::{
    Cancellable, CodeAction, CompletionItem, Diagnostic, DocumentSymbol, FileId, FilePosition,
    FoldRange, HoverResult, InlayHint, SignatureHelp,
};

mod features;
mod semantic;

/// The single mutable owner of analysis state — one per project/workspace.
///
/// The input world is a virtual file system (`FileId` → UTF-8 text); the host never
/// reads paths. Clients push text via [`AnalysisHost::apply_change`].
#[derive(Debug, Clone, Default)]
pub struct AnalysisHost {
    files: Arc<FxHashMap<FileId, Arc<str>>>,
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

    /// Apply a batch of input changes. The **only** mutation entry point.
    pub fn apply_change(&mut self, change: Change) {
        let files = Arc::make_mut(&mut self.files);
        for (id, text) in change.files {
            match text {
                Some(t) => {
                    files.insert(id, t);
                }
                None => {
                    files.remove(&id);
                }
            }
        }
    }

    /// A cheap, cloneable, `Send` snapshot for read queries.
    #[must_use]
    pub fn analysis(&self) -> Analysis {
        Analysis {
            files: Arc::clone(&self.files),
        }
    }
}

/// An immutable snapshot of the world. Every query is `Cancellable` (Phase 1 never
/// actually cancels, but the type is on the surface for the Phase-3 salsa swap).
#[derive(Debug, Clone)]
pub struct Analysis {
    files: Arc<FxHashMap<FileId, Arc<str>>>,
}

impl Analysis {
    fn text(&self, file: FileId) -> Option<&str> {
        self.files.get(&file).map(|t| &**t)
    }

    // ---- Tier-0 features: real data ----

    /// A pretty-printed dump of the syntax tree (debugging / playground).
    ///
    /// # Errors
    /// Currently infallible; returns `Err(Cancelled)` only once Phase 3 wires real
    /// cancellation.
    pub fn syntax_tree(&self, file: FileId) -> Cancellable<Option<String>> {
        Ok(self
            .text(file)
            .map(|t| gdscript_syntax::parse(t).debug_tree()))
    }

    /// Parse-error diagnostics ∪ the Phase-2 §5 type diagnostics.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn diagnostics(&self, file: FileId) -> Cancellable<Vec<Diagnostic>> {
        Ok(self
            .text(file)
            .map(|t| {
                let mut diags = features::diagnostics(t);
                diags.extend(semantic::type_diagnostics(t));
                diags
            })
            .unwrap_or_default())
    }

    /// The document outline (classes, funcs, vars, consts, enums, signals, members).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn document_symbols(&self, file: FileId) -> Cancellable<Vec<DocumentSymbol>> {
        Ok(self
            .text(file)
            .map(features::document_symbols)
            .unwrap_or_default())
    }

    /// Foldable ranges (blocks, `#region` pairs, multi-line brackets).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn folding_ranges(&self, file: FileId) -> Cancellable<Vec<FoldRange>> {
        Ok(self
            .text(file)
            .map(features::folding_ranges)
            .unwrap_or_default())
    }

    /// Completions. After `receiver.` it offers the inferred member set; otherwise (or when
    /// the receiver is `Variant`/`Unknown`) it falls back to the Tier-0 by-name completion
    /// (keywords, annotations after `@`, document-local symbols) so it never regresses.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn completions(&self, pos: FilePosition) -> Cancellable<Vec<CompletionItem>> {
        Ok(self
            .text(pos.file)
            .map(|t| {
                semantic::member_completions(t, pos.offset)
                    .unwrap_or_else(|| features::completions(t, pos.offset))
            })
            .unwrap_or_default())
    }

    /// Hover: the inferred type of the expression / binding under the cursor (`Unknown`
    /// elided). `None` when there is nothing typed there.
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn hover(&self, pos: FilePosition) -> Cancellable<Option<HoverResult>> {
        Ok(self
            .text(pos.file)
            .and_then(|t| semantic::hover(t, pos.offset)))
    }

    /// Inlay `: T` hints on `:=` declarations + unannotated params / `for`-vars (suppressed
    /// when the type is `Variant`/`Unknown`).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn inlay_hints(&self, file: FileId) -> Cancellable<Vec<InlayHint>> {
        Ok(self
            .text(file)
            .map(semantic::inlay_hints)
            .unwrap_or_default())
    }

    /// Signature help at a call site (active parameter by top-level comma count).
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn signature_help(&self, pos: FilePosition) -> Cancellable<Option<SignatureHelp>> {
        Ok(self
            .text(pos.file)
            .and_then(|t| semantic::signature_help(t, pos.offset)))
    }

    /// Code actions at a position (currently "add type annotation").
    ///
    /// # Errors
    /// See [`Analysis::syntax_tree`].
    pub fn code_actions(&self, pos: FilePosition) -> Cancellable<Vec<CodeAction>> {
        Ok(self
            .text(pos.file)
            .map(|t| semantic::code_actions(t, pos.offset, pos.file))
            .unwrap_or_default())
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
        Ok(Vec::new())
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
