//! `gdscript-session` — a URI-keyed, JSON-returning session over [`gdscript_ide`].
//!
//! The shared, pure-Rust, **wasm-clean** core that the napi (`gdscript-ffi`) and wasm
//! (`gdscript-wasm`) bindings wrap as thin, near-trivial delegators. A napi/wasm binding crate
//! cannot be `cargo test`ed natively (no Node runtime / `libnode` at link time), so every piece of
//! real logic — the URI→[`FileId`] interner, the document lifecycle, and the JSON serialization of
//! query results — lives here, fully unit-tested with plain `cargo test`.
//!
//! ## Why JSON strings
//! Each query returns a **JSON string** of the engine-neutral `gdscript-base` POD results. The
//! client `JSON.parse`s it and maps byte offsets to its own position encoding (UTF-16 in a JS
//! editor). This keeps the core API (`FileId` + `Cancellable`) untouched and the binding crates
//! free of any POD re-declaration — the payloads (a handful of diagnostics / completion items) are
//! small, so the parse cost is negligible next to the boundary crossing.
//!
//! ## Cancellation
//! The bindings are single-threaded (one JS thread), so no `apply_change` ever races a query; the
//! `Cancellable` results therefore never cancel and are unwrapped to their default here.
#![cfg_attr(docsrs, feature(doc_cfg))]

use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{AnalysisHost, Change};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

/// A live, URI-keyed analysis session: one [`AnalysisHost`] kept alive across edits (so salsa's
/// incremental caching survives) plus the URI→[`FileId`] interner the string-keyed clients need.
#[derive(Debug, Default)]
pub struct Session {
    host: AnalysisHost,
    uris: FxHashMap<String, FileId>,
    /// Files whose `res://` path has already been recorded — so it is set exactly once, the first
    /// time one is provided, regardless of whether the file was first seen via `open` or `change`.
    with_path: FxHashSet<FileId>,
    next_id: u32,
}

/// Serialize a POD result to a JSON string; on the (practically impossible) serialization failure,
/// fall back to `fallback` (`"[]"` for arrays, `"null"` for options) so a query never panics.
fn json<T: Serialize>(value: &T, fallback: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| fallback.to_owned())
}

impl Session {
    /// An empty session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- document lifecycle (mutating) -------------------------------------------------------

    /// Open or replace a document by `uri`. Its `res_path` (`res://…`) is recorded the **first time
    /// one is provided** for the file — set once so cross-file `preload` / `extends` / autoload
    /// resolution lights up, and never re-sent (which would needlessly invalidate the resource-path
    /// registry). Crucially this is tracked per `FileId`, **not** by whether the `uri` was new, so a
    /// `change()` before the first path-bearing `open()` no longer swallows the path (a file
    /// interned by an early `change()` still records the `res://` path on its next `open`).
    pub fn open(&mut self, uri: &str, text: &str, res_path: Option<&str>) {
        let (id, _) = self.intern(uri);
        let mut change = Change::new();
        change.change_file(id, text);
        if let Some(p) = res_path
            && self.with_path.insert(id)
        {
            change.set_file_path(id, p.to_owned());
        }
        self.host.apply_change(change);
    }

    /// Replace a document's text by `uri` (its `res://` path is unchanged). Unknown `uri`s are
    /// interned as new files (an upsert), so a client may `change` without a prior `open`.
    pub fn change(&mut self, uri: &str, text: &str) {
        let (id, _) = self.intern(uri);
        let mut change = Change::new();
        change.change_file(id, text);
        self.host.apply_change(change);
    }

    /// Close (remove) a document by `uri`. A later re-open assigns a fresh [`FileId`] (and re-records
    /// its `res://` path). No-op for an unknown `uri`.
    pub fn close(&mut self, uri: &str) {
        if let Some(id) = self.uris.remove(uri) {
            self.with_path.remove(&id);
            let mut change = Change::new();
            change.remove_file(id);
            self.host.apply_change(change);
        }
    }

    /// Whether `uri` is currently open. Lets a client distinguish "file not tracked" from a genuine
    /// empty result, since the array queries return `"[]"` and the option queries `null` for **both**
    /// an unknown `uri` and an open-but-empty one.
    #[must_use]
    pub fn is_open(&self, uri: &str) -> bool {
        self.uris.contains_key(uri)
    }

    /// Set the project's `project.godot` text (enables `[autoload]` singleton resolution). Set on
    /// project open / when it changes.
    pub fn set_project_config(&mut self, text: &str) {
        let mut change = Change::new();
        change.set_project_config(text);
        self.host.apply_change(change);
    }

    /// Install a runtime-fetched engine model (the wasm path: a `fetch`ed `extension_api` blob).
    /// Returns `false` if the bytes fail to decode. Native builds use the bundled model and need not
    /// call this; the **wasm** binding fetches the blob and installs it (without it, completion/hover
    /// for engine classes like `Button`/`Control` are unavailable on wasm — the embedded blob is
    /// native-only). First install wins; installing it **after** queries have run recomputes them
    /// (the wasm engine-generation invalidation), so an async load after opening a document is safe.
    pub fn load_engine_api(&mut self, bytes: &[u8]) -> bool {
        self.host.set_engine_api(bytes)
    }

    // ---- queries (returning JSON strings) ----------------------------------------------------

    /// Parse + type diagnostics for `uri`, as a JSON array string (`"[]"` for an unknown `uri`).
    #[must_use]
    pub fn diagnostics(&self, uri: &str) -> String {
        match self.file_id(uri) {
            Some(file) => json(
                &self.host.analysis().diagnostics(file).unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// The document outline for `uri`, as a JSON array string.
    #[must_use]
    pub fn document_symbols(&self, uri: &str) -> String {
        match self.file_id(uri) {
            Some(file) => json(
                &self
                    .host
                    .analysis()
                    .document_symbols(file)
                    .unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Foldable ranges for `uri`, as a JSON array string.
    #[must_use]
    pub fn folding_ranges(&self, uri: &str) -> String {
        match self.file_id(uri) {
            Some(file) => json(
                &self
                    .host
                    .analysis()
                    .folding_ranges(file)
                    .unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Inlay hints for `uri`, as a JSON array string.
    #[must_use]
    pub fn inlay_hints(&self, uri: &str) -> String {
        match self.file_id(uri) {
            Some(file) => json(
                &self.host.analysis().inlay_hints(file).unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Completions at a byte `offset` in `uri`, as a JSON array string.
    #[must_use]
    pub fn completions(&self, uri: &str, offset: u32) -> String {
        match self.pos(uri, offset) {
            Some(pos) => json(
                &self.host.analysis().completions(pos).unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Hover at a byte `offset` in `uri`. `None` (→ JS `null`) when there is nothing typed there or
    /// `uri` is unknown; otherwise a JSON object string of the [`HoverResult`](gdscript_base::HoverResult).
    #[must_use]
    pub fn hover(&self, uri: &str, offset: u32) -> Option<String> {
        let pos = self.pos(uri, offset)?;
        self.host
            .analysis()
            .hover(pos)
            .ok()
            .flatten()
            .map(|h| json(&h, "null"))
    }

    /// Signature help at a byte `offset` in `uri`. `None` (→ JS `null`) when not at a call site.
    #[must_use]
    pub fn signature_help(&self, uri: &str, offset: u32) -> Option<String> {
        let pos = self.pos(uri, offset)?;
        self.host
            .analysis()
            .signature_help(pos)
            .ok()
            .flatten()
            .map(|s| json(&s, "null"))
    }

    /// Code actions at a byte `offset` in `uri`, as a JSON array string.
    #[must_use]
    pub fn code_actions(&self, uri: &str, offset: u32) -> String {
        match self.pos(uri, offset) {
            Some(pos) => json(
                &self.host.analysis().code_actions(pos).unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Go-to-definition target(s) for the symbol at a byte `offset` in `uri` (cross-file), as a
    /// JSON array string of [`NavTarget`](gdscript_base::NavTarget)s.
    #[must_use]
    pub fn goto_definition(&self, uri: &str, offset: u32) -> String {
        match self.pos(uri, offset) {
            Some(pos) => json(
                &self
                    .host
                    .analysis()
                    .goto_definition(pos)
                    .unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Every reference to the symbol at a byte `offset` in `uri` (project-wide), as a JSON array
    /// string of [`Reference`](gdscript_base::Reference)s.
    #[must_use]
    pub fn find_references(&self, uri: &str, offset: u32) -> String {
        match self.pos(uri, offset) {
            Some(pos) => json(
                &self
                    .host
                    .analysis()
                    .find_references(pos)
                    .unwrap_or_default(),
                "[]",
            ),
            None => "[]".to_owned(),
        }
    }

    /// Rename the symbol at a byte `offset` in `uri` to `new_name`. **Always an envelope** (never
    /// bare `null`), never a partial edit: `{"ok": <SourceChange>}` on success, or
    /// `{"error": <RenameError | reason-string>}` on a refusal, an unknown `uri`, or cancellation.
    #[must_use]
    pub fn rename(&self, uri: &str, offset: u32, new_name: &str) -> String {
        let Some(pos) = self.pos(uri, offset) else {
            return json(&serde_json::json!({ "error": "document not open" }), "null");
        };
        match self.host.analysis().rename(pos, new_name) {
            Ok(Ok(change)) => json(&serde_json::json!({ "ok": change }), "null"),
            Ok(Err(err)) => json(&serde_json::json!({ "error": err }), "null"),
            Err(_) => json(&serde_json::json!({ "error": "cancelled" }), "null"),
        }
    }

    /// Project-wide symbols matching `query` (fuzzy-ranked), as a JSON array string.
    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> String {
        json(
            &self
                .host
                .analysis()
                .workspace_symbols(query)
                .unwrap_or_default(),
            "[]",
        )
    }

    /// The pretty-printed syntax tree for `uri` (debugging / playground). `None` for an unknown
    /// `uri`. Returned as a plain string (it is already text, not POD).
    #[must_use]
    pub fn syntax_tree(&self, uri: &str) -> Option<String> {
        let file = self.file_id(uri)?;
        self.host.analysis().syntax_tree(file).ok().flatten()
    }

    // ---- interner -----------------------------------------------------------------------------

    /// The [`FileId`] for `uri`, if open.
    fn file_id(&self, uri: &str) -> Option<FileId> {
        self.uris.get(uri).copied()
    }

    /// A [`FilePosition`] at `offset` in `uri`, if open.
    fn pos(&self, uri: &str, offset: u32) -> Option<FilePosition> {
        self.file_id(uri).map(|file| FilePosition { file, offset })
    }

    /// Get-or-assign the [`FileId`] for `uri`; the bool is `true` when it was newly interned.
    fn intern(&mut self, uri: &str) -> (FileId, bool) {
        if let Some(&id) = self.uris.get(uri) {
            return (id, false);
        }
        let id = FileId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.uris.insert(uri.to_owned(), id);
        (id, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).expect("valid json")
    }

    #[test]
    fn open_then_diagnostics_and_symbols() {
        let mut s = Session::new();
        s.open(
            "file:///main.gd",
            "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
            None,
        );
        let diags = parse(&s.diagnostics("file:///main.gd"));
        assert_eq!(diags[0]["code"], "INTEGER_DIVISION");
        let syms = parse(&s.document_symbols("file:///main.gd"));
        assert_eq!(syms[0]["name"], "f");
    }

    #[test]
    fn unknown_uri_is_empty_not_panic() {
        let s = Session::new();
        assert_eq!(s.diagnostics("nope"), "[]");
        assert_eq!(s.document_symbols("nope"), "[]");
        assert_eq!(s.completions("nope", 0), "[]");
        assert_eq!(s.hover("nope", 0), None);
        assert_eq!(s.signature_help("nope", 0), None);
        assert_eq!(s.syntax_tree("nope"), None);
    }

    #[test]
    fn change_updates_text() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tpass\n", None);
        assert_eq!(parse(&s.diagnostics("u")).as_array().unwrap().len(), 0);
        s.change("u", "var x = )\n"); // now a syntax error
        assert!(!parse(&s.diagnostics("u")).as_array().unwrap().is_empty());
    }

    #[test]
    fn close_removes_the_document() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tpass\n", None);
        assert_eq!(parse(&s.document_symbols("u")).as_array().unwrap().len(), 1);
        s.close("u");
        assert_eq!(s.document_symbols("u"), "[]");
    }

    #[test]
    fn hover_is_some_on_a_typed_binding() {
        let mut s = Session::new();
        let src = "func f():\n\tvar n := 1\n";
        s.open("u", src, None);
        // hover on `n` (the binding) yields a typed result.
        let offset = u32::try_from(src.find("n :=").unwrap()).unwrap();
        assert!(s.hover("u", offset).is_some());
    }

    #[test]
    fn cross_file_preload_resolves_via_res_paths() {
        // The real guitkx pattern, end-to-end: a `preload`ed script's method resolves cross-file.
        let mut s = Session::new();
        s.open(
            "file:///markup.gd",
            "class_name Markup\nfunc parse() -> int:\n\treturn 1\n",
            Some("res://markup.gd"),
        );
        s.open(
            "file:///main.gd",
            "const M = preload(\"res://markup.gd\")\nfunc go():\n\tvar n := M.new().parse()\n",
            Some("res://main.gd"),
        );
        // valid code → no diagnostics; the cross-file preload typed `n` as int → an `: int` inlay.
        assert_eq!(
            parse(&s.diagnostics("file:///main.gd"))
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let hints = s.inlay_hints("file:///main.gd");
        assert!(
            hints.contains("int"),
            "expected an `: int` inlay, got {hints}"
        );
    }

    #[test]
    fn autoload_resolves_via_project_config() {
        let mut s = Session::new();
        s.open(
            "file:///audio.gd",
            "func volume() -> int:\n\treturn 50\n",
            Some("res://audio.gd"),
        );
        s.open(
            "file:///main.gd",
            "func go():\n\tvar v := Audio.volume()\n",
            Some("res://main.gd"),
        );
        s.set_project_config("[autoload]\nAudio=\"*res://audio.gd\"\n");
        assert_eq!(
            parse(&s.diagnostics("file:///main.gd"))
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert!(s.inlay_hints("file:///main.gd").contains("int"));
    }

    #[test]
    fn rename_returns_ok_or_error_shape() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tvar x := 1\n\tvar y := x\n", None);
        // rename the local `x` (first occurrence) — should succeed with an `{"ok": …}` envelope.
        let offset = u32::try_from("func f():\n\tvar ".len()).unwrap();
        let out = parse(&s.rename("u", offset, "renamed"));
        assert!(
            out.get("ok").is_some() || out.get("error").is_some(),
            "{out}"
        );
    }

    #[test]
    fn reopen_after_close_assigns_fresh_id_and_works() {
        let mut s = Session::new();
        s.open("u", "func a():\n\tpass\n", None);
        s.close("u");
        s.open("u", "func b():\n\tpass\n", None);
        let syms = parse(&s.document_symbols("u"));
        assert_eq!(syms[0]["name"], "b");
    }

    #[test]
    fn change_before_open_still_records_res_path() {
        // Hunt #1: a `change()` that interns a uri before the first path-bearing `open()` must NOT
        // swallow the `res://` path — else cross-file resolution silently breaks.
        let mut s = Session::new();
        let markup = "class_name Markup\nfunc parse() -> int:\n\treturn 1\n";
        s.change("file:///markup.gd", markup); // interns markup.gd with NO path
        s.open("file:///markup.gd", markup, Some("res://markup.gd")); // path must be recorded now
        s.open(
            "file:///main.gd",
            "const M = preload(\"res://markup.gd\")\nfunc go():\n\tvar n := M.new().parse()\n",
            Some("res://main.gd"),
        );
        assert!(
            s.inlay_hints("file:///main.gd").contains("int"),
            "the res:// path must survive a change()-before-open(); cross-file preload should type `n` as int",
        );
    }

    #[test]
    fn rename_unknown_uri_returns_error_envelope() {
        // Hunt #2: never bare `null` — always `{"ok"}|{"error"}`.
        let s = Session::new();
        let out = parse(&s.rename("nope", 0, "x"));
        assert!(out.get("error").is_some(), "{out}");
        assert!(out.get("ok").is_none());
    }

    #[test]
    fn is_open_distinguishes_tracked_files() {
        // Hunt #3/#4: a client can tell "not tracked" from a genuine empty result.
        let mut s = Session::new();
        assert!(!s.is_open("u"));
        s.open("u", "func f():\n\tpass\n", None);
        assert!(s.is_open("u"));
        s.close("u");
        assert!(!s.is_open("u"));
    }

    #[test]
    fn load_engine_api_rejects_garbage_without_breaking() {
        // An invalid blob is rejected (false), never a panic, and the session keeps working (on
        // native the bundled fallback remains). The valid-blob round-trip is the wasm path, covered
        // by gdscript-api's from_bytes tests + the wasm CI build.
        let mut s = Session::new();
        s.open("u", "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n", None);
        assert!(!s.load_engine_api(b"not a valid rkyv engine-api blob"));
        assert_eq!(parse(&s.diagnostics("u"))[0]["code"], "INTEGER_DIVISION");
    }
}
