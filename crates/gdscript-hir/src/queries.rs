//! The salsa-tracked entry points for the semantic layer, layered on `gdscript-db`'s
//! [`parse`](gdscript_db::parse) query.
//!
//! These are the memoized queries the IDE layer drives. The heavy lifting stays in
//! [`crate::item_tree`] / [`crate::infer`] as plain `(parsed file) -> value` functions; this
//! module only wraps them so their results are cached per revision and recomputed
//! incrementally. [`item_tree`] is the **firewall** query (Playbook §4): it reads only the
//! parse, never a function body, so an unchanged set of signatures backdates across body edits.
//!
//! Phase-3 note: cross-file resolution (the `resolve_external` seam) is still `Ty::Unknown`
//! here — M1 threads `&dyn Db` + `FileId` into inference to light it up. M0 only swaps the
//! cache engine; the single-file results are byte-identical.

use std::sync::Arc;

use gdscript_db::{Db, FileText, SourceRoot, parse};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::infer::FileInference;
use crate::item_tree::ItemTree;

/// The item tree for `file` (signatures only — the body-edit firewall). Memoized; recomputes
/// when the parse changes but backdates when the resulting signatures are unchanged.
#[salsa::tracked]
pub fn item_tree(db: &dyn Db, file: FileText) -> Arc<ItemTree> {
    crate::item_tree::item_tree(&parse(db, file).syntax_node())
}

/// Whole-file inference for `file`. With no engine model available (`wasm32`, until the host
/// wires the fetched blob in) this is an empty result — matching the Phase-2 graceful path.
#[salsa::tracked]
pub fn analyze_file(db: &dyn Db, file: FileText) -> Arc<FileInference> {
    match db.engine() {
        Some(api) => Arc::new(crate::infer::analyze_file(
            db,
            api,
            &parse(db, file).syntax_node(),
        )),
        None => Arc::new(FileInference::default()),
    }
}

/// The project-wide global `class_name` registry: each registered name → the file declaring it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalRegistry {
    classes: FxHashMap<SmolStr, FileText>,
}

impl GlobalRegistry {
    /// The file declaring `name` as a global `class_name`, if any.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<FileText> {
        self.classes.get(name).copied()
    }

    /// The number of registered global classes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    /// Whether no global class is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

/// A file's `class_name`, if it declares one — the **offset-free projection** of its item tree
/// that [`global_registry`] depends on. It reads only `item_tree(file).class_name` (never a byte
/// range), so a body edit re-runs `item_tree` but this query *backdates* (its value is
/// unchanged), leaving the registry — and everything cross-file — undisturbed by a keystroke.
#[salsa::tracked]
pub fn file_class_name(db: &dyn Db, file: FileText) -> Option<SmolStr> {
    item_tree(db, file).class_name.clone()
}

/// The project-wide global `class_name` registry. Keyed on the [`SourceRoot`] file-set input and
/// the per-file [`file_class_name`] projections. A duplicate `class_name` keeps the first by
/// `FileId` order (the file set is sorted), so resolution is deterministic. (Collision
/// diagnostics are an M1 follow-up.)
#[salsa::tracked]
pub fn global_registry(db: &dyn Db, root: SourceRoot) -> Arc<GlobalRegistry> {
    let mut classes = FxHashMap::default();
    for &file in root.files(db) {
        if let Some(name) = file_class_name(db, file) {
            classes.entry(name).or_insert(file);
        }
    }
    Arc::new(GlobalRegistry { classes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_base::FileId;
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    fn db_with(src: &str) -> (RootDatabase, FileText) {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), src, Durability::LOW);
        let ft = db.file_text(FileId(0)).unwrap();
        (db, ft)
    }

    #[test]
    fn tracked_item_tree_matches_the_plain_fn() {
        let (db, ft) = db_with("class_name Foo\nfunc f():\n\tpass\n");
        let tree = item_tree(&db, ft);
        assert_eq!(tree.class_name.as_deref(), Some("Foo"));
        // Memoized: a second query is the same Arc value.
        assert_eq!(item_tree(&db, ft), tree);
    }

    #[test]
    fn tracked_analyze_file_runs_inference() {
        let (db, ft) = db_with("func add(a: int, b: int) -> int:\n\treturn a + b\n");
        let fi = analyze_file(&db, ft);
        // The engine model is present on native, so inference produced a unit.
        assert!(!fi.units.is_empty());
        assert!(fi.diagnostics.is_empty());
    }

    // ---- the body-edit firewall (the M0 CI gate, Playbook §4) -----------------------------
    //
    // A query that reads only `item_tree` (signatures) must NOT recompute when a function body
    // changes: editing a body changes the parse, `item_tree` re-validates but its value is
    // unchanged, so salsa BACKDATES it and dependents are spared. We witness this with a counter
    // bumped inside a signature-only tracked query (a standard salsa test idiom — the counter is
    // test-only impurity that does not affect the result). `class_name_witness` is also the seed
    // of M1's global `class_name` registry.

    use std::sync::atomic::{AtomicU32, Ordering};

    static WITNESS_RUNS: AtomicU32 = AtomicU32::new(0);

    /// Depends ONLY on `item_tree` (never on a body). Counts its own executions.
    #[salsa::tracked]
    fn class_name_witness(db: &dyn gdscript_db::Db, file: FileText) -> Option<smol_str::SmolStr> {
        WITNESS_RUNS.fetch_add(1, Ordering::SeqCst);
        item_tree(db, file).class_name.clone()
    }

    #[test]
    fn body_edit_does_not_invalidate_signature_queries() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Foo\nfunc f():\n\tvar a := 1\n",
            Durability::LOW,
        );
        let ft = db.file_text(FileId(0)).unwrap();

        // Warm the cache.
        assert_eq!(class_name_witness(&db, ft).as_deref(), Some("Foo"));
        let runs_after_warm = WITNESS_RUNS.load(Ordering::SeqCst);

        // Edit ONLY a function body, keeping byte length (`1` -> `2`): signatures are unchanged,
        // so `item_tree` backdates and the firewall holds.
        db.set_file_text(
            FileId(0),
            "class_name Foo\nfunc f():\n\tvar a := 2\n",
            Durability::LOW,
        );
        assert_eq!(class_name_witness(&db, ft).as_deref(), Some("Foo"));

        assert_eq!(
            WITNESS_RUNS.load(Ordering::SeqCst),
            runs_after_warm,
            "REGRESSION: a body edit re-ran a signature-only query — the item_tree firewall broke",
        );
    }

    #[test]
    fn global_registry_resolves_class_names_across_files() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Player\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "class_name Enemy\nvar hp := 10\n",
            Durability::LOW,
        );
        db.set_file_text(FileId(2), "func no_class():\n\tpass\n", Durability::LOW);
        db.sync_source_root();
        let root = db.source_root().unwrap();

        let reg = global_registry(&db, root);
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.resolve("Player"), db.file_text(FileId(0)));
        assert_eq!(reg.resolve("Enemy"), db.file_text(FileId(1)));
        assert!(reg.resolve("Nonexistent").is_none());
    }

    // The TRUE downstream firewall (the M1 reframe of the pinned M0 limitation): a body edit must
    // not invalidate the project-wide registry. `file_class_name` is offset-free, so even a
    // *length-changing* body edit — which shifts `item_tree`'s byte ranges and forces it to
    // re-execute — leaves `file_class_name` backdating (its value, the class name, is unchanged).
    // The registry, and every consumer of it, is therefore untouched by a keystroke.

    static REGISTRY_OBSERVED: AtomicU32 = AtomicU32::new(0);

    /// Test-only consumer of the registry; re-runs iff the registry's value actually changes.
    #[salsa::tracked]
    fn observe_registry(db: &dyn gdscript_db::Db, root: SourceRoot) -> usize {
        REGISTRY_OBSERVED.fetch_add(1, Ordering::SeqCst);
        global_registry(db, root).len()
    }

    #[test]
    fn body_edit_does_not_invalidate_the_global_registry() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Player\nfunc f():\n\tvar a := 1\n",
            Durability::LOW,
        );
        db.set_file_text(FileId(1), "class_name Enemy\n", Durability::LOW);
        db.sync_source_root();
        let root = db.source_root().unwrap();

        assert_eq!(observe_registry(&db, root), 2);
        let runs = REGISTRY_OBSERVED.load(Ordering::SeqCst);

        // A length-CHANGING body edit (`1` -> `123456`) — NO sync_source_root (a body edit is not
        // a structure change). The class name is unchanged, so the registry must not recompute.
        db.set_file_text(
            FileId(0),
            "class_name Player\nfunc f():\n\tvar a := 123456\n",
            Durability::LOW,
        );

        assert_eq!(observe_registry(&db, root), 2);
        assert_eq!(
            REGISTRY_OBSERVED.load(Ordering::SeqCst),
            runs,
            "REGRESSION: a body edit re-ran a global_registry consumer — the cross-file firewall broke",
        );
    }
}
