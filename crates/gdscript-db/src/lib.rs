//! `gdscript-db` — the input layer for the analyzer.
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
#[salsa::db]
pub trait Db: salsa::Database {
    /// The text input for `file`, or `None` if no text has been set for it.
    fn file_text(&self, file: FileId) -> Option<FileText>;
    /// The bundled engine model, or `None` on `wasm32` (no embedded blob — the host wires the
    /// fetched blob in via `EngineApi::from_bytes` in Phase 5).
    fn engine(&self) -> Option<&'static EngineApi>;
}

/// The VFS leaf: one file's UTF-8 text, as a salsa input, plus its [`FileId`] (so a query
/// holding only a `FileText` can recover the id for cross-file resolution).
#[salsa::input(debug)]
pub struct FileText {
    /// The file's full text (interned `Arc<str>`; the getter returns `&Arc<str>`).
    #[returns(ref)]
    pub text: Arc<str>,
    /// The opaque file id this text belongs to.
    pub file_id: FileId,
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
                let ft = FileText::builder(Arc::from(text), file)
                    .durability(durability)
                    .new(db);
                vac.insert(ft);
            }
        }
    }

    /// Drop `file` from the side table (its salsa input lingers, unreferenced, until GC).
    pub fn remove(&self, file: FileId) {
        self.inner.remove(&file);
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

    /// Remove `file`'s entry from the side table.
    pub fn remove_file(&mut self, file: FileId) {
        self.files.remove(file);
    }
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn file_text(&self, file: FileId) -> Option<FileText> {
        self.files.file_text(file)
    }

    // The native arm is always `Some`; only the `wasm32` arm is `None`. clippy sees one target.
    #[allow(clippy::unnecessary_wraps)]
    fn engine(&self) -> Option<&'static EngineApi> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Some(gdscript_api::bundled())
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
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
}
