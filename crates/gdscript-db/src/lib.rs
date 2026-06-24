//! `gdscript-db` — the input layer for the analyzer.
//!
//! Holds the virtual file system (`FileId` → text, always injected — never `std::fs`), the
//! project model, and (from Phase 3) the **salsa** query graph: `#[salsa::input]`s set via
//! `apply_change`, `#[salsa::tracked]` derived queries, durability tiers. The Phase-0/1/2
//! plain VFS map + reparse-on-change is being replaced here, localized behind the unchanged
//! `gdscript-ide` public API (Playbook §3.M0).
//!
//! Crate boundary: `gdscript-db` is the *base* of the salsa stack — it owns the `Db` trait,
//! the inputs, and the `parse` query (it may depend on `gdscript-syntax`, never on
//! `gdscript-hir`). The higher queries (`item_tree`, `analyze_file`) live in `gdscript-hir`,
//! which depends on this crate for `&dyn Db`. This one-way layering is what avoids a
//! `db ↔ hir` dependency cycle.
//!
//! Must build for `wasm32` (single-threaded; salsa with `default-features = false`).
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use gdscript_syntax::Parse;

/// The database trait `gdscript-hir` / `gdscript-ide` depend on. `#[salsa::db]` on the *trait*
/// makes it a salsa supertrait, so any `&dyn Db` upcasts to `&dyn salsa::Database` and every
/// `#[salsa::tracked]` free function downstream can take `db: &dyn Db`.
//
// M0 commit 1 (spike): the trait carries no methods yet — the FileId side-map + `engine()` /
// `project_config()` accessors land in commit 2.
#[salsa::db]
pub trait Db: salsa::Database {}

/// The VFS leaf: one file's UTF-8 text, as a salsa input.
///
/// `FileId` is deliberately **not** itself a salsa input — the `FileId → FileText` mapping is a
/// side table owned by the database (commit 2), mirroring rust-analyzer's `base-db`. M0 commit
/// 1 exercises a single `FileText` directly.
#[salsa::input]
pub struct FileText {
    /// The file's full text (interned `Arc<str>`; the getter returns `&Arc<str>`).
    #[returns(ref)]
    pub text: Arc<str>,
}

/// Parse a file to its lossless CST.
///
/// **The storability spike (M0 commit 1):** this proves a `cstree`-green-tree-bearing [`Parse`]
/// is a valid `#[salsa::tracked]` return value. The function's result is `Clone`; backdating it
/// (so an unchanged reparse doesn't bump the revision) additionally needs `Eq` on [`Parse`] —
/// resolved empirically here.
#[salsa::tracked]
pub fn parse(db: &dyn Db, file: FileText) -> Parse {
    gdscript_syntax::parse(file.text(db))
}

/// The concrete analyzer database — a salsa `Storage` plus (from commit 2) the file side-table.
#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
}

// `salsa::Storage` is not `Debug`, but the public `AnalysisHost`/`Analysis` that will own a
// `RootDatabase` must stay `Debug` (frozen API); hand-impl an opaque one.
impl std::fmt::Debug for RootDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootDatabase").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_returns_a_cst() {
        let db = RootDatabase::default();
        let ft = FileText::new(&db, Arc::from("func f():\n\tpass\n"));
        let p = parse(&db, ft);
        assert!(p.errors().is_empty());
        // Re-querying the same input returns the memoized value (no re-parse).
        let p2 = parse(&db, ft);
        assert_eq!(p.debug_tree(), p2.debug_tree());
    }
}
