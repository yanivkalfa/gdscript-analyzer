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

use gdscript_db::{Db, FileText, ProjectConfig, SourceRoot, parse};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use gdscript_base::FileId;

use crate::infer::FileInference;
use crate::item_tree::{ItemTree, Member};
use crate::ty::{ScriptRefId, Ty};

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

    /// Every registered `(class_name, declaring file)` pair (for workspace symbols).
    pub fn iter(&self) -> impl Iterator<Item = (&SmolStr, FileText)> + '_ {
        self.classes.iter().map(|(k, v)| (k, *v))
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

/// The project-wide `res:// path → FileId` registry (M3): the map `preload("res://x.gd")` and
/// `extends "res://x.gd"` resolve through. Keyed on the [`SourceRoot`] file-set input and each
/// file's `res_path` salsa-input field. `res_path` is a *separate* input field from `text`
/// (salsa tracks input fields individually), so this registry **backdates across body edits**
/// exactly like [`global_registry`] — a keystroke never rebuilds it. A duplicate path keeps the
/// first by `FileId` order (the file set is sorted), matching `global_registry`'s policy.
#[salsa::tracked]
pub fn res_path_registry(db: &dyn Db, root: SourceRoot) -> Arc<FxHashMap<SmolStr, FileId>> {
    let mut map = FxHashMap::default();
    for &file in root.files(db) {
        if let Some(path) = file.res_path(db) {
            map.entry(path).or_insert_with(|| file.file_id(db));
        }
    }
    Arc::new(map)
}

/// The project's autoload **singletons** (`*`-flagged `[autoload]` entries) — the bare names that
/// resolve as globals in code. Maps each singleton name → its resource path (M4). Non-singleton
/// autoloads are deliberately excluded (loaded-but-not-global). Keyed on [`ProjectConfig`] alone
/// (it iterates only the config text), so it backdates across every `.gd` keystroke.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoloadRegistry {
    singletons: FxHashMap<SmolStr, SmolStr>,
}

impl AutoloadRegistry {
    /// The resource path of the singleton autoload named `name`, if any.
    #[must_use]
    pub fn resolve_path(&self, name: &str) -> Option<&SmolStr> {
        self.singletons.get(name)
    }

    /// The number of registered singleton autoloads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.singletons.len()
    }

    /// Whether no singleton autoload is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.singletons.is_empty()
    }
}

/// The project-wide autoload-singleton registry, parsed from `project.godot` (M4). Only
/// `*`-flagged entries become globals; a duplicate name keeps the first (deterministic).
#[salsa::tracked]
pub fn autoload_registry(db: &dyn Db, config: ProjectConfig) -> Arc<AutoloadRegistry> {
    let mut singletons = FxHashMap::default();
    for e in crate::project::parse_autoloads(config.project_godot_text(db)) {
        if e.is_singleton {
            singletons.entry(e.name).or_insert(e.path);
        }
    }
    Arc::new(AutoloadRegistry { singletons })
}

/// One member of a script class, as a cross-file reference sees it (a resolved type, never a
/// byte range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberSig {
    /// A method — its resolved return type.
    Method(Ty),
    /// A `var` / `const` — its resolved type.
    Field(Ty),
    /// A signal.
    Signal,
}

/// A script class's own members, by name, plus its resolved `extends` base — the **offset-free
/// projection** a cross-file reference resolves against. Reads only `item_tree` signatures (+
/// annotation/base resolution), never bodies or byte ranges, so it backdates on body edits (the
/// cross-file firewall). Member lookup walks the base chain (M2): own members here, inherited
/// ones via [`base`](ScriptClass::base).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptClass {
    members: FxHashMap<SmolStr, MemberSig>,
    base: Ty,
}

impl ScriptClass {
    /// The signature of the member named `name`, if the class declares one *itself* (not
    /// inherited — the caller walks [`base`](ScriptClass::base) for inherited members).
    #[must_use]
    pub fn member(&self, name: &str) -> Option<&MemberSig> {
        self.members.get(name)
    }

    /// The resolved `extends` base: an engine `Object`, a user `ScriptRef`, or `Unknown`.
    #[must_use]
    pub fn base(&self) -> &Ty {
        &self.base
    }
}

/// The `class_name` behind a [`ScriptRef`](crate::ty::Ty::ScriptRef), for display (hover /
/// inlay). `Ty::label` cannot resolve this on its own — it has only the engine model, not the
/// project registry.
#[must_use]
pub fn script_ref_name(db: &dyn Db, sref: ScriptRefId) -> Option<SmolStr> {
    let file = db.file_text(FileId(sref.0))?;
    file_class_name(db, file)
}

/// The member table of the script in `file`. Member types are resolved against the engine model
/// and the registry (a member typed as another `class_name` resolves to its `ScriptRef`).
#[salsa::tracked]
pub fn script_class(db: &dyn Db, file: FileText) -> Arc<ScriptClass> {
    let tree = item_tree(db, file);
    let Some(api) = db.engine() else {
        return Arc::new(ScriptClass {
            members: FxHashMap::default(),
            base: Ty::Unknown,
        });
    };
    let resolve_ann = |ann: Option<&str>| -> Ty {
        ann.map_or(Ty::Variant, |t| {
            crate::resolve::resolve_type_name(db, api, t)
        })
    };
    let mut members = FxHashMap::default();
    for m in &tree.members {
        let Some(name) = m.name() else { continue };
        let sig = match m {
            Member::Func(f) => MemberSig::Method(resolve_ann(f.return_type.as_deref())),
            Member::Var(v) => MemberSig::Field(resolve_ann(v.type_ref.as_deref())),
            Member::Const(c) => MemberSig::Field(resolve_ann(c.type_ref.as_deref())),
            Member::Signal(_) => MemberSig::Signal,
            // Enums + inner classes aren't modeled as instance members yet (M2+).
            Member::Enum(_) | Member::Class(_) => continue,
        };
        members.insert(SmolStr::new(name), sig);
    }
    // The resolved `extends` base — a user `ScriptRef` (another class_name / "res://…") walks
    // into the inheritance chain; an engine `Object` ends it at the API table.
    let base = crate::resolve::resolve_base(db, api, &tree);
    Arc::new(ScriptClass { members, base })
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

    #[test]
    fn cross_file_class_name_member_resolves() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Widget\nfunc make() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "func use_it():\n\tvar w := Widget.make()\n",
            Durability::LOW,
        );
        db.sync_source_root();

        let file1 = db.file_text(FileId(1)).unwrap();
        let fi = analyze_file(&db, file1);
        let api = db.engine().unwrap();

        // `w := Widget.make()` resolves `Widget` (a cross-file class_name) to its ScriptRef, then
        // its `make` method to its `int` return type.
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("a unit with a binding");
        assert_eq!(
            unit.result.bindings[0].ty.label(api).as_deref(),
            Some("int")
        );
        assert!(
            fi.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            fi.diagnostics
        );
    }

    #[test]
    fn unknown_member_on_script_ref_is_seam_not_warning() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Widget\nfunc make() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "func use_it():\n\tWidget.not_a_member()\n",
            Durability::LOW,
        );
        db.sync_source_root();

        let file1 = db.file_text(FileId(1)).unwrap();
        let fi = analyze_file(&db, file1);
        // A member we don't model is the seam (Unknown) — never UNSAFE_METHOD_ACCESS.
        assert!(
            fi.diagnostics.is_empty(),
            "a missing member on a ScriptRef must not warn: {:?}",
            fi.diagnostics
        );
    }

    #[test]
    fn inherited_members_resolve_through_user_and_engine_bases() {
        let mut db = RootDatabase::default();
        // Derived -> Base (user) -> Node (engine) -> … -> Object.
        db.set_file_text(
            FileId(0),
            "class_name Base\nextends Node\nfunc base_method() -> int:\n\treturn 1\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "class_name Derived\nextends Base\nfunc own() -> String:\n\treturn \"x\"\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(2),
            "func use_it():\n\tvar d: Derived\n\tvar own := d.own()\n\tvar from_base := d.base_method()\n\tvar from_engine := d.get_instance_id()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| u.result.bindings.len() >= 4)
            .expect("use_it unit with 4 bindings");
        // [0]=d, [1]=own (own member), [2]=base_method (user base), [3]=get_instance_id (engine base).
        assert_eq!(
            unit.result.bindings[1].ty.label(api).as_deref(),
            Some("String")
        );
        assert_eq!(
            unit.result.bindings[2].ty.label(api).as_deref(),
            Some("int")
        );
        assert_eq!(
            unit.result.bindings[3].ty.label(api).as_deref(),
            Some("int")
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn cyclic_extends_terminates() {
        let mut db = RootDatabase::default();
        // A extends B extends A — illegal in Godot, but the member walk must not loop.
        db.set_file_text(FileId(0), "class_name A\nextends B\n", Durability::LOW);
        db.set_file_text(FileId(1), "class_name B\nextends A\n", Durability::LOW);
        db.set_file_text(
            FileId(2),
            "func use_it():\n\tvar a: A\n\tvar x := a.nope()\n",
            Durability::LOW,
        );
        db.sync_source_root();

        // Must terminate (depth cap) — a.nope() walks A->B->A->… and bottoms out at the seam.
        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    // ---- M3: res:// path map + preload / extends "res://…" const-aliasing -----------------

    /// Add a file with both its text and its `res://` path (the loader's add-time pair).
    fn set_with_path(db: &mut RootDatabase, id: u32, path: &str, src: &str) {
        db.set_file_text(FileId(id), src, Durability::LOW);
        db.set_file_path(FileId(id), path);
    }

    #[test]
    fn res_path_registry_maps_paths_to_files() {
        let mut db = RootDatabase::default();
        set_with_path(&mut db, 0, "res://a.gd", "class_name A\n");
        set_with_path(&mut db, 1, "res://sub/b.gd", "func f():\n\tpass\n");
        db.set_file_text(FileId(2), "func no_path():\n\tpass\n", Durability::LOW); // no res:// path
        db.sync_source_root();
        let root = db.source_root().unwrap();

        let reg = res_path_registry(&db, root);
        assert_eq!(reg.get("res://a.gd"), Some(&FileId(0)));
        assert_eq!(reg.get("res://sub/b.gd"), Some(&FileId(1)));
        assert!(reg.get("res://missing.gd").is_none());
        // A file with no path contributes nothing.
        assert_eq!(reg.len(), 2);
    }

    // The res:// path firewall: a body edit must not rebuild the path registry. `res_path` is a
    // *separate* salsa-input field from `text`, so even a length-changing body edit (which
    // re-runs `item_tree`) leaves `res_path` — and the registry — untouched.

    static RES_REGISTRY_OBSERVED: AtomicU32 = AtomicU32::new(0);

    #[salsa::tracked]
    fn observe_res_registry(db: &dyn gdscript_db::Db, root: SourceRoot) -> usize {
        RES_REGISTRY_OBSERVED.fetch_add(1, Ordering::SeqCst);
        res_path_registry(db, root).len()
    }

    #[test]
    fn body_edit_does_not_invalidate_the_res_path_registry() {
        let mut db = RootDatabase::default();
        set_with_path(&mut db, 0, "res://a.gd", "func f():\n\tvar a := 1\n");
        db.sync_source_root();
        let root = db.source_root().unwrap();

        assert_eq!(observe_res_registry(&db, root), 1);
        let runs = RES_REGISTRY_OBSERVED.load(Ordering::SeqCst);

        // Length-CHANGING body edit, NO path re-set, NO sync_source_root: the path is unchanged,
        // so the registry must not recompute.
        db.set_file_text(FileId(0), "func f():\n\tvar a := 123456\n", Durability::LOW);

        assert_eq!(observe_res_registry(&db, root), 1);
        assert_eq!(
            RES_REGISTRY_OBSERVED.load(Ordering::SeqCst),
            runs,
            "REGRESSION: a body edit re-ran a res_path_registry consumer — the path firewall broke",
        );
    }

    #[test]
    fn preload_const_resolves_to_script_ref_members() {
        let mut db = RootDatabase::default();
        set_with_path(
            &mut db,
            0,
            "res://widget.gd",
            "class_name Widget\nfunc make() -> int:\n\treturn 5\nconst MAX := 10\n",
        );
        set_with_path(
            &mut db,
            1,
            "res://main.gd",
            "const W = preload(\"res://widget.gd\")\nfunc use_it():\n\tvar a := W.make()\n\tvar b := W.new()\n",
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| u.result.bindings.len() >= 2)
            .expect("use_it unit with 2 bindings");
        // W.make() → int; W.new() → an instance of Widget (a ScriptRef).
        assert_eq!(
            unit.result.bindings[0].ty.label(api).as_deref(),
            Some("int")
        );
        assert!(
            matches!(unit.result.bindings[1].ty, Ty::ScriptRef(_)),
            "W.new() should be a script instance, got {:?}",
            unit.result.bindings[1].ty
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn preload_of_script_without_class_name_resolves() {
        // The key distinction from M1: preload resolves by PATH, so a script with *no* class_name
        // (absent from the global_registry) is still resolved.
        let mut db = RootDatabase::default();
        set_with_path(
            &mut db,
            0,
            "res://helper.gd",
            "func help() -> String:\n\treturn \"x\"\n",
        );
        set_with_path(
            &mut db,
            1,
            "res://main.gd",
            "func use_it():\n\tvar h := preload(\"res://helper.gd\")\n\tvar s := h.help()\n",
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| u.result.bindings.len() >= 2)
            .expect("use_it unit");
        assert!(
            matches!(unit.result.bindings[0].ty, Ty::ScriptRef(_)),
            "preload of a class_name-less script must still resolve: {:?}",
            unit.result.bindings[0].ty
        );
        assert_eq!(
            unit.result.bindings[1].ty.label(api).as_deref(),
            Some("String")
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn extends_res_path_inherits_members() {
        let mut db = RootDatabase::default();
        // base.gd has NO class_name — reachable only by its res:// path.
        set_with_path(
            &mut db,
            0,
            "res://base.gd",
            "extends Node\nfunc base_method() -> int:\n\treturn 1\n",
        );
        set_with_path(
            &mut db,
            1,
            "res://derived.gd",
            "class_name Derived\nextends \"res://base.gd\"\nfunc own() -> String:\n\treturn \"x\"\n",
        );
        set_with_path(
            &mut db,
            2,
            "res://main.gd",
            "func use_it():\n\tvar d: Derived\n\tvar a := d.own()\n\tvar b := d.base_method()\n\tvar c := d.get_instance_id()\n",
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| u.result.bindings.len() >= 4)
            .expect("use_it unit with 4 bindings");
        // own() (own member), base_method() (via the res:// user base), get_instance_id() (the
        // engine base behind base.gd).
        assert_eq!(
            unit.result.bindings[1].ty.label(api).as_deref(),
            Some("String")
        );
        assert_eq!(
            unit.result.bindings[2].ty.label(api).as_deref(),
            Some("int")
        );
        assert_eq!(
            unit.result.bindings[3].ty.label(api).as_deref(),
            Some("int")
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn dangling_preload_is_seam_not_panic() {
        let mut db = RootDatabase::default();
        set_with_path(
            &mut db,
            0,
            "res://main.gd",
            "func use_it():\n\tvar x := preload(\"res://does_not_exist.gd\")\n\tx.whatever()\n",
        );
        db.sync_source_root();
        // An unresolvable path → the seam (Unknown): no diagnostic, no panic.
        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn load_literal_stays_opaque_not_aliased_to_preload() {
        let mut db = RootDatabase::default();
        set_with_path(
            &mut db,
            0,
            "res://widget.gd",
            "class_name Widget\nfunc make() -> int:\n\treturn 5\n",
        );
        set_with_path(
            &mut db,
            1,
            "res://main.gd",
            "func use_it():\n\tvar w := load(\"res://widget.gd\")\n",
        );
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("use_it unit");
        // `load(...)` is an ordinary runtime call returning an opaque Resource — it must NOT be
        // aliased to `preload` (no script ScriptRef, no static `.new()` typing).
        assert!(
            !matches!(unit.result.bindings[0].ty, Ty::ScriptRef(_)),
            "load() must stay opaque, not alias preload: {:?}",
            unit.result.bindings[0].ty
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn is_narrows_to_a_user_class_cross_file() {
        // `if x is Widget:` narrows `x` to the user `ScriptRef`, so `x.make()` resolves to its
        // cross-file return type — the is/as-over-user-types path (already works once ScriptRef
        // is informative; M4 just gates it). `int` here PROVES narrowing: without it `x` stays
        // Variant and `x.make()` would be Variant.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Widget\nfunc make() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "func use_it(x):\n\tif x is Widget:\n\t\tvar n := x.make()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        // (bindings include the param `x`; assert *some* binding — the `n` one — is int.)
        assert!(
            fi.units
                .iter()
                .flat_map(|u| &u.result.bindings)
                .any(|b| b.ty.label(api).as_deref() == Some("int")),
            "`x.make()` after `is Widget` should narrow + resolve to int",
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn as_casts_to_a_user_class_cross_file() {
        // `(x as Widget).make()` types the cast as the user `ScriptRef`, so `.make()` → int.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Widget\nfunc make() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "func use_it(x):\n\tvar n := (x as Widget).make()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        assert!(
            fi.units
                .iter()
                .flat_map(|u| &u.result.bindings)
                .any(|b| b.ty.label(api).as_deref() == Some("int")),
            "`(x as Widget).make()` should resolve to int",
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn renaming_a_files_path_reindexes_the_registry() {
        // A path change (rename) DOES update the registry (it is not a body edit).
        let mut db = RootDatabase::default();
        set_with_path(&mut db, 0, "res://old.gd", "class_name A\n");
        db.sync_source_root();
        let root = db.source_root().unwrap();
        assert_eq!(
            res_path_registry(&db, root).get("res://old.gd"),
            Some(&FileId(0))
        );

        db.set_file_path(FileId(0), "res://new.gd");
        let root = db.source_root().unwrap();
        let reg = res_path_registry(&db, root);
        assert_eq!(reg.get("res://new.gd"), Some(&FileId(0)));
        assert!(reg.get("res://old.gd").is_none());
    }

    // ---- M4: autoloads (project.godot [autoload]) + is/as widen-only narrowing --------------

    #[test]
    fn star_autoload_gdscript_resolves_as_global_and_members() {
        let mut db = RootDatabase::default();
        // `game.gd` has NO class_name — the autoload resolves it by PATH (not the class registry).
        db.set_file_text(
            FileId(0),
            "func score() -> int:\n\treturn 0\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://game.gd");
        db.set_file_text(
            FileId(1),
            "func f():\n\tvar s := Game.score()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.set_project_config("[autoload]\nGame=\"*res://game.gd\"\n");
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("f unit");
        // `Game` (a *-singleton) resolves to its ScriptRef; `Game.score()` → int.
        assert_eq!(
            unit.result.bindings[0].ty.label(api).as_deref(),
            Some("int")
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn non_star_autoload_is_not_a_global() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "func score() -> int:\n\treturn 0\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://game.gd");
        db.set_file_text(
            FileId(1),
            "func f():\n\tvar s := Game.score()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        // No leading `*` → loaded-but-not-global; the bare name `Game` must NOT resolve.
        db.set_project_config("[autoload]\nGame=\"res://game.gd\"\n");
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("f unit");
        // `Game` → seam (Unknown), so `s` is uninformative (no `int`); and NO diagnostic.
        assert_eq!(unit.result.bindings[0].ty.label(api), None);
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn tscn_autoload_is_the_seam_never_false_warns() {
        let mut db = RootDatabase::default();
        // A scene (`.tscn`) autoload: typing it `Node` would false-warn on the root script's own
        // members, so it stays the seam (scene-root typing is Phase 4).
        db.set_file_text(FileId(0), "func f():\n\tHud.play_song()\n", Durability::LOW);
        db.set_file_path(FileId(0), "res://main.gd");
        db.set_project_config("[autoload]\nHud=\"*res://hud.tscn\"\n");
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        // `Hud.play_song()` on a seam receiver → no diagnostic (no false UNSAFE_METHOD_ACCESS).
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    // The autoload firewall: a `.gd` body edit must not rebuild the autoload registry, which is
    // keyed only on the `ProjectConfig` input (not on file text).

    static AUTOLOAD_OBSERVED: AtomicU32 = AtomicU32::new(0);

    #[salsa::tracked]
    fn observe_autoload_registry(db: &dyn gdscript_db::Db, config: ProjectConfig) -> usize {
        AUTOLOAD_OBSERVED.fetch_add(1, Ordering::SeqCst);
        autoload_registry(db, config).len()
    }

    #[test]
    fn autoload_registry_firewalled_against_body_edits() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "func f():\n\tvar a := 1\n", Durability::LOW);
        db.set_file_path(FileId(0), "res://game.gd");
        db.set_project_config("[autoload]\nGame=\"*res://game.gd\"\n");
        db.sync_source_root();
        let config = db.project_config().unwrap();

        assert_eq!(observe_autoload_registry(&db, config), 1);
        let runs = AUTOLOAD_OBSERVED.load(Ordering::SeqCst);

        // Length-changing `.gd` body edit, NO set_project_config: the autoload registry must not
        // recompute (its sole input — ProjectConfig — is untouched).
        db.set_file_text(FileId(0), "func f():\n\tvar a := 999999\n", Durability::LOW);

        assert_eq!(observe_autoload_registry(&db, config), 1);
        assert_eq!(
            AUTOLOAD_OBSERVED.load(Ordering::SeqCst),
            runs,
            "REGRESSION: a body edit re-ran an autoload_registry consumer — the config firewall broke",
        );
    }

    #[test]
    fn is_userbase_narrows_to_derived_but_not_un_narrowed_to_base() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Base\nfunc base_m() -> int:\n\treturn 1\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "class_name Derived\nextends Base\nfunc own_m() -> String:\n\treturn \"x\"\n",
            Durability::LOW,
        );
        // (a) untyped `x` + `is Derived` → narrow to Derived → `x.own_m()` resolves (String).
        // (b) `d: Derived` + `is Base` → widen-only: d STAYS Derived → `d.own_m()` resolves (String).
        db.set_file_text(
            FileId(2),
            "func use_it(x):\n\tif x is Derived:\n\t\tvar a := x.own_m()\n\tvar d: Derived\n\tif d is Base:\n\t\tvar b := d.own_m()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let strings = fi
            .units
            .iter()
            .flat_map(|u| &u.result.bindings)
            .filter(|b| b.ty.label(api).as_deref() == Some("String"))
            .count();
        // Both `own_m()` calls resolve to String: proves narrow-to-Derived AND no un-narrow-to-Base.
        assert!(
            strings >= 2,
            "expected both own_m() calls to type as String (narrow-down + widen-only), got {strings}",
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }
}
