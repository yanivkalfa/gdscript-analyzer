//! `gdscript-session` — a URI-keyed session over [`gdscript_ide`] that returns structured
//! [`serde_json::Value`]s.
//!
//! The shared, pure-Rust, **wasm-clean** core that the napi (`gdscript-ffi`) and wasm
//! (`gdscript-wasm`) bindings wrap as thin, near-trivial delegators. A napi/wasm binding crate
//! cannot be `cargo test`ed natively (no Node runtime / `libnode` at link time), so every piece of
//! real logic — the URI→[`FileId`] interner, the document lifecycle, and the serialization of
//! query results — lives here, fully unit-tested with plain `cargo test`.
//!
//! ## Why structured values (not JSON strings)
//! Each query returns a [`serde_json::Value`] of the engine-neutral `gdscript-base` POD results, not
//! a JSON *string*. The bindings hand it straight to the JS engine as a **native object** — napi via
//! its `serde-json` feature, wasm via `serde_wasm_bindgen` (json-compatible) — so the client no
//! longer `JSON.parse`s. This keeps the core API (`FileId` + `Cancellable`) untouched and the binding
//! crates free of any POD re-declaration; the payloads (a handful of diagnostics / completion items)
//! are small, so the intermediate `Value` is negligible next to the boundary crossing. The client
//! still maps byte offsets to its own position encoding (UTF-16 in a JS editor).
//!
//! ## URI enrichment (mirror-free navigation)
//! `gdscript-base` navigation/edit POD identifies files by [`FileId`] (an opaque `u32`). Because the
//! session owns the URI↔`FileId` interner, it injects a `"uri"` string next to **every** `"file"`
//! key in a serialized result ([`Session::enrich_uris`]) — `NavTarget`, `Reference`, `FileEdit`, the
//! `FileRange` inside a `WouldCollide` rename error, and anything FileId-bearing added later. A
//! client therefore resolves cross-file targets directly, without maintaining its own `FileId`→URI
//! mirror.
//!
//! ## Cancellation
//! The bindings are single-threaded (one JS thread), so no `apply_change` ever races a query; the
//! `Cancellable` results therefore never cancel and are unwrapped to their default here.
#![cfg_attr(docsrs, feature(doc_cfg))]

use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{AnalysisHost, Change};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use serde_json::Value;

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

/// An empty JSON array — the result for an unknown `uri` (so an array query never panics and a
/// client can distinguish it via [`Session::is_open`]).
fn empty_array() -> Value {
    Value::Array(Vec::new())
}

/// Recursively inject a `"uri"` string next to every `"file": <id>` key, resolving the id through
/// `by_id`. A generic walk (not per-type) so any FileId-bearing POD gains a `uri` uniformly — now
/// (`NavTarget`, `Reference`, `FileEdit`, a `WouldCollide` `FileRange`) and as the POD grows. Ids
/// absent from `by_id` (a file closed since the query) are left without a `uri`.
fn inject_uris(value: &mut Value, by_id: &FxHashMap<u32, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                inject_uris(item, by_id);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                inject_uris(child, by_id);
            }
            // Extract the id first (ending the immutable borrow) before inserting `uri`.
            let file = map
                .get("file")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            if let Some(id) = file
                && let Some(uri) = by_id.get(&id)
            {
                map.insert("uri".to_owned(), Value::String(uri.clone()));
            }
        }
        _ => {}
    }
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

    // ---- queries (returning `serde_json::Value`) ---------------------------------------------

    /// Parse + type diagnostics for `uri` (a JSON array; `[]` for an unknown `uri`).
    #[must_use]
    pub fn diagnostics(&self, uri: &str) -> Value {
        match self.file_id(uri) {
            Some(file) => {
                self.enriched(&self.host.analysis().diagnostics(file).unwrap_or_default())
            }
            None => empty_array(),
        }
    }

    /// The document outline for `uri` (a JSON array).
    #[must_use]
    pub fn document_symbols(&self, uri: &str) -> Value {
        match self.file_id(uri) {
            Some(file) => self.enriched(
                &self
                    .host
                    .analysis()
                    .document_symbols(file)
                    .unwrap_or_default(),
            ),
            None => empty_array(),
        }
    }

    /// Foldable ranges for `uri` (a JSON array).
    #[must_use]
    pub fn folding_ranges(&self, uri: &str) -> Value {
        match self.file_id(uri) {
            Some(file) => self.enriched(
                &self
                    .host
                    .analysis()
                    .folding_ranges(file)
                    .unwrap_or_default(),
            ),
            None => empty_array(),
        }
    }

    /// Inlay hints for `uri` (a JSON array).
    #[must_use]
    pub fn inlay_hints(&self, uri: &str) -> Value {
        match self.file_id(uri) {
            Some(file) => {
                self.enriched(&self.host.analysis().inlay_hints(file).unwrap_or_default())
            }
            None => empty_array(),
        }
    }

    /// Completions at a byte `offset` in `uri` (a JSON array).
    #[must_use]
    pub fn completions(&self, uri: &str, offset: u32) -> Value {
        match self.pos(uri, offset) {
            Some(pos) => self.enriched(&self.host.analysis().completions(pos).unwrap_or_default()),
            None => empty_array(),
        }
    }

    /// Hover at a byte `offset` in `uri`. `None` (→ JS `null`) when there is nothing typed there or
    /// `uri` is unknown; otherwise a JSON object of the [`HoverResult`](gdscript_base::HoverResult).
    #[must_use]
    pub fn hover(&self, uri: &str, offset: u32) -> Option<Value> {
        let pos = self.pos(uri, offset)?;
        self.host
            .analysis()
            .hover(pos)
            .ok()
            .flatten()
            .map(|h| self.enriched(&h))
    }

    /// Signature help at a byte `offset` in `uri`. `None` (→ JS `null`) when not at a call site.
    #[must_use]
    pub fn signature_help(&self, uri: &str, offset: u32) -> Option<Value> {
        let pos = self.pos(uri, offset)?;
        self.host
            .analysis()
            .signature_help(pos)
            .ok()
            .flatten()
            .map(|s| self.enriched(&s))
    }

    /// Code actions at a byte `offset` in `uri` (a JSON array).
    #[must_use]
    pub fn code_actions(&self, uri: &str, offset: u32) -> Value {
        match self.pos(uri, offset) {
            Some(pos) => self.enriched(&self.host.analysis().code_actions(pos).unwrap_or_default()),
            None => empty_array(),
        }
    }

    /// Go-to-definition target(s) for the symbol at a byte `offset` in `uri` (cross-file), as a JSON
    /// array of [`NavTarget`](gdscript_base::NavTarget)s — each carrying a `uri` (see crate docs).
    #[must_use]
    pub fn goto_definition(&self, uri: &str, offset: u32) -> Value {
        match self.pos(uri, offset) {
            Some(pos) => self.enriched(
                &self
                    .host
                    .analysis()
                    .goto_definition(pos)
                    .unwrap_or_default(),
            ),
            None => empty_array(),
        }
    }

    /// Every reference to the symbol at a byte `offset` in `uri` (project-wide), as a JSON array of
    /// [`Reference`](gdscript_base::Reference)s — each carrying a `uri` (see crate docs).
    #[must_use]
    pub fn find_references(&self, uri: &str, offset: u32) -> Value {
        match self.pos(uri, offset) {
            Some(pos) => self.enriched(
                &self
                    .host
                    .analysis()
                    .find_references(pos)
                    .unwrap_or_default(),
            ),
            None => empty_array(),
        }
    }

    /// Rename the symbol at a byte `offset` in `uri` to `new_name`. **Always an envelope** (never
    /// bare `null`), never a partial edit: `{"ok": <SourceChange>}` on success, or
    /// `{"error": <RenameError | reason-string>}` on a refusal, an unknown `uri`, or cancellation.
    /// Each `FileEdit` in a successful change (and the `FileRange` in a `WouldCollide` error) carries
    /// a `uri` (see crate docs).
    #[must_use]
    pub fn rename(&self, uri: &str, offset: u32, new_name: &str) -> Value {
        let Some(pos) = self.pos(uri, offset) else {
            return serde_json::json!({ "error": "document not open" });
        };
        let mut out = match self.host.analysis().rename(pos, new_name) {
            Ok(Ok(change)) => serde_json::json!({ "ok": change }),
            Ok(Err(err)) => serde_json::json!({ "error": err }),
            Err(_) => serde_json::json!({ "error": "cancelled" }),
        };
        self.enrich_uris(&mut out);
        out
    }

    /// Project-wide symbols matching `query` (fuzzy-ranked), as a JSON array.
    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> Value {
        self.enriched(
            &self
                .host
                .analysis()
                .workspace_symbols(query)
                .unwrap_or_default(),
        )
    }

    /// The pretty-printed syntax tree for `uri` (debugging / playground). `None` for an unknown
    /// `uri`. Returned as a plain string (it is already text, not POD).
    #[must_use]
    pub fn syntax_tree(&self, uri: &str) -> Option<String> {
        let file = self.file_id(uri)?;
        self.host.analysis().syntax_tree(file).ok().flatten()
    }

    // ---- serialization ------------------------------------------------------------------------

    /// Serialize a POD result to a [`serde_json::Value`], then [`enrich`](Self::enrich_uris) it with
    /// `uri`s. On the practically-impossible serialization failure, falls back to `null`.
    fn enriched<T: Serialize>(&self, pod: &T) -> Value {
        let mut value = serde_json::to_value(pod).unwrap_or(Value::Null);
        self.enrich_uris(&mut value);
        value
    }

    /// Inject a `"uri"` next to every `"file"` key in `value`, resolved through the reverse interner
    /// (`FileId`→URI). Built per query — the open-file count is small. See the crate-level docs.
    fn enrich_uris(&self, value: &mut Value) {
        let by_id: FxHashMap<u32, String> = self
            .uris
            .iter()
            .map(|(uri, id)| (id.0, uri.clone()))
            .collect();
        inject_uris(value, &by_id);
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
    use serde_json::json;

    #[test]
    fn open_then_diagnostics_and_symbols() {
        let mut s = Session::new();
        s.open(
            "file:///main.gd",
            "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
            None,
        );
        let diags = s.diagnostics("file:///main.gd");
        assert_eq!(diags[0]["code"], "INTEGER_DIVISION");
        let syms = s.document_symbols("file:///main.gd");
        assert_eq!(syms[0]["name"], "f");
    }

    #[test]
    fn unknown_uri_is_empty_not_panic() {
        let s = Session::new();
        assert_eq!(s.diagnostics("nope"), json!([]));
        assert_eq!(s.document_symbols("nope"), json!([]));
        assert_eq!(s.completions("nope", 0), json!([]));
        assert_eq!(s.hover("nope", 0), None);
        assert_eq!(s.signature_help("nope", 0), None);
        assert_eq!(s.syntax_tree("nope"), None);
    }

    #[test]
    fn change_updates_text() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tpass\n", None);
        assert_eq!(s.diagnostics("u").as_array().unwrap().len(), 0);
        s.change("u", "var x = )\n"); // now a syntax error
        assert!(!s.diagnostics("u").as_array().unwrap().is_empty());
    }

    #[test]
    fn close_removes_the_document() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tpass\n", None);
        assert_eq!(s.document_symbols("u").as_array().unwrap().len(), 1);
        s.close("u");
        assert_eq!(s.document_symbols("u"), json!([]));
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
            "const M = preload(\"res://markup.gd\")\nfunc go():\n\tvar n := M.new().parse()\n\treturn n\n",
            Some("res://main.gd"),
        );
        // valid code → no diagnostics; the cross-file preload typed `n` as int → an `: int` inlay.
        assert_eq!(
            s.diagnostics("file:///main.gd").as_array().unwrap().len(),
            0
        );
        let hints = s.inlay_hints("file:///main.gd");
        assert!(
            hints.to_string().contains("int"),
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
            "func go():\n\tvar v := Audio.volume()\n\treturn v\n",
            Some("res://main.gd"),
        );
        s.set_project_config("[autoload]\nAudio=\"*res://audio.gd\"\n");
        assert_eq!(
            s.diagnostics("file:///main.gd").as_array().unwrap().len(),
            0
        );
        assert!(s.inlay_hints("file:///main.gd").to_string().contains("int"));
    }

    #[test]
    fn rename_returns_ok_or_error_shape() {
        let mut s = Session::new();
        s.open("u", "func f():\n\tvar x := 1\n\tvar y := x\n", None);
        // rename the local `x` (first occurrence) — should succeed with an `{"ok": …}` envelope.
        let offset = u32::try_from("func f():\n\tvar ".len()).unwrap();
        let out = s.rename("u", offset, "renamed");
        assert!(
            out.get("ok").is_some() || out.get("error").is_some(),
            "{out}"
        );
    }

    #[test]
    fn navigation_results_carry_uri() {
        // Typed-FFI: a client resolves cross-file targets directly — every `file` id is mirrored by
        // a `uri`, so no `FileId`→URI mirror is needed on the client side.
        let mut s = Session::new();
        let src = "func f():\n\tvar x := 1\n\tvar y := x\n";
        s.open("file:///a.gd", src, None);
        let offset = u32::try_from(src.find("x :=").unwrap()).unwrap();
        let refs = s.find_references("file:///a.gd", offset);
        let arr = refs.as_array().expect("an array of references");
        assert!(!arr.is_empty(), "expected references, got {refs}");
        assert!(
            arr.iter().all(|r| r["uri"] == "file:///a.gd"),
            "every reference carries its uri: {refs}"
        );
    }

    #[test]
    fn rename_edits_carry_uri() {
        // The successful-rename envelope's per-file edits also carry a `uri` (mirror-free apply).
        let mut s = Session::new();
        let src = "func f():\n\tvar x := 1\n\tvar y := x\n";
        s.open("file:///a.gd", src, None);
        let offset = u32::try_from(src.find("x :=").unwrap()).unwrap();
        let out = s.rename("file:///a.gd", offset, "renamed");
        if let Some(edits) = out["ok"]["edits"].as_array() {
            assert!(!edits.is_empty(), "{out}");
            assert!(
                edits.iter().all(|e| e["uri"] == "file:///a.gd"),
                "every FileEdit carries its uri: {out}"
            );
        } else {
            // If the engine refuses to rename a local, the contract is still an error envelope.
            assert!(out.get("error").is_some(), "{out}");
        }
    }

    #[test]
    fn reopen_after_close_assigns_fresh_id_and_works() {
        let mut s = Session::new();
        s.open("u", "func a():\n\tpass\n", None);
        s.close("u");
        s.open("u", "func b():\n\tpass\n", None);
        let syms = s.document_symbols("u");
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
            "const M = preload(\"res://markup.gd\")\nfunc go():\n\tvar n := M.new().parse()\n\treturn n\n",
            Some("res://main.gd"),
        );
        assert!(
            s.inlay_hints("file:///main.gd").to_string().contains("int"),
            "the res:// path must survive a change()-before-open(); cross-file preload should type `n` as int",
        );
    }

    #[test]
    fn rename_unknown_uri_returns_error_envelope() {
        // Hunt #2: never bare `null` — always `{"ok"}|{"error"}`.
        let s = Session::new();
        let out = s.rename("nope", 0, "x");
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
        assert_eq!(s.diagnostics("u")[0]["code"], "INTEGER_DIVISION");
    }
}
