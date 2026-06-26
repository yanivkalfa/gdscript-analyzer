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
use gdscript_scene::{NodeIdx, SceneModel};

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
            file.file_id(db),
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

// ---- M1: scenes (.tscn/.tres) ------------------------------------------------------------

/// Whether a `res://` path is a *text* scene/resource we parse (`.tscn`/`.tres`). Binary
/// `.scn`/`.res` are detected-and-degraded by the parser, but we don't waste a parse on a `.gd`.
fn is_scene_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("");
    ext.eq_ignore_ascii_case("tscn") || ext.eq_ignore_ascii_case("tres")
}

/// The parsed [`SceneModel`] for `file` (M1) — memoized; recomputes only when the file text
/// changes. A non-scene file (a `.gd`, or no `res://` path) yields an empty model (so the query is
/// total). The pure `gdscript_scene::parse_scene` is the cache body; this just wraps + gates it.
#[salsa::tracked]
pub fn scene_model(db: &dyn Db, file: FileText) -> Arc<SceneModel> {
    let is_scene = file.res_path(db).as_deref().is_some_and(is_scene_path);
    if is_scene {
        Arc::new(gdscript_scene::parse_scene(file.text(db)))
    } else {
        Arc::new(gdscript_scene::parse_scene(""))
    }
}

/// Where a script (`.gd`) is attached in a scene: the owning scene file + the node carrying the
/// `script = ExtResource(...)`. `$Path` in that script resolves relative to this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneAttach {
    /// The owning scene's file.
    pub scene: FileId,
    /// The node the script attaches to (the `$`-path base).
    pub node: NodeIdx,
    /// Whether the script attaches to **more than one** scene (the first kept here). When `true`,
    /// a `$Path` valid in another scene must not be flagged `INVALID_NODE_PATH` (no false positive).
    pub ambiguous: bool,
}

/// The project-wide **script → owning scene** index (M1): each `.gd`'s `res://` path → the (first)
/// scene + node that attaches it. Built by scanning every scene's `ext_resources` for a
/// `type="Script"` reference. Keyed on the [`SourceRoot`] file-set + each scene file's text (via
/// [`scene_model`]); a `.gd` **body** edit never touches a `.tscn` text, so this **backdates across
/// `.gd` keystrokes** — the firewall (a scene edit correctly invalidates it). A duplicate (one
/// script in many scenes) keeps the first by `FileId` order (the slice's single-scene policy).
#[salsa::tracked]
pub fn script_scene_index(db: &dyn Db, root: SourceRoot) -> Arc<FxHashMap<SmolStr, SceneAttach>> {
    let mut map: FxHashMap<SmolStr, SceneAttach> = FxHashMap::default();
    for &file in root.files(db) {
        if !file.res_path(db).as_deref().is_some_and(is_scene_path) {
            continue;
        }
        let model = scene_model(db, file);
        let scene = file.file_id(db);
        for (i, node) in model.nodes.iter().enumerate() {
            let Some(script_id) = node.script.as_ref() else {
                continue;
            };
            let Some(path) = model
                .ext_resources
                .get(script_id)
                .and_then(|e| e.path.clone())
            else {
                continue;
            };
            let node = NodeIdx(u32::try_from(i).unwrap_or(u32::MAX));
            match map.get_mut(&path) {
                // already attached by an earlier scene → ambiguous (keep the first).
                Some(existing) => existing.ambiguous = true,
                None => {
                    map.insert(
                        path,
                        SceneAttach {
                            scene,
                            node,
                            ambiguous: false,
                        },
                    );
                }
            }
        }
    }
    Arc::new(map)
}

/// The owning-scene context for the script in `file` (M1): the scene's [`FileId`], the parsed
/// scene, and the attach node, so `$Path`/`%Unique`/`get_node("…")` can resolve (and go-to-def can
/// jump into the `.tscn`). `None` when the project has no scene attaching this script (the
/// overwhelmingly common single-file / dynamic-UI case → node paths stay `Node`).
#[must_use]
pub fn scene_context(db: &dyn Db, file: FileText) -> Option<SceneContext> {
    let res_path = file.res_path(db)?;
    let root = db.source_root()?;
    let attach = *script_scene_index(db, root).get(res_path.as_str())?;
    let scene_file = db.file_text(attach.scene)?;
    Some(SceneContext {
        scene: attach.scene,
        model: scene_model(db, scene_file),
        attach: attach.node,
        ambiguous: attach.ambiguous,
    })
}

/// The resolved owning-scene context for a script — the scene file, its model, the attach node, and
/// whether the attachment is ambiguous (multi-scene). Returned by [`scene_context`].
#[derive(Debug, Clone)]
pub struct SceneContext {
    /// The owning scene's file.
    pub scene: FileId,
    /// The parsed scene model.
    pub model: Arc<SceneModel>,
    /// The node the script attaches to (the `$`-path base).
    pub attach: NodeIdx,
    /// Whether the script attaches to multiple scenes (suppresses `INVALID_NODE_PATH`).
    pub ambiguous: bool,
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
    fn non_gd_preload_resource_stays_seam() {
        // A `preload` of a non-`.gd` resource must NOT resolve to a script `ScriptRef`, even if the
        // path is in the res:// registry — typing a `.tscn`/PackedScene as a script would wrongly
        // accept `.new()`/member access (scene-root typing is Phase 4). Defensive gate (the loader
        // indexes only `.gd` today, but a future scene-ingesting loader must not mis-type this).
        let mut db = RootDatabase::default();
        set_with_path(&mut db, 0, "res://scene.tscn", "class_name SceneRoot\n");
        set_with_path(
            &mut db,
            1,
            "res://main.gd",
            "func f():\n\tvar s := preload(\"res://scene.tscn\")\n",
        );
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("f unit");
        assert!(
            !matches!(unit.result.bindings[0].ty, Ty::ScriptRef(_)),
            "a non-.gd preload must stay the seam, got {:?}",
            unit.result.bindings[0].ty
        );
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
    fn star_autoload_scene_resolves_via_its_root_script() {
        // A `*`-autoload pointing at a `.tscn` whose root has an attached script resolves to that
        // script (the singleton-scene pattern) — `Music.volume()` → int, no false UNSAFE. This was
        // deferred to Phase 4 (scene ingestion); now closed.
        let mut db = RootDatabase::default();
        // music.gd (no class_name — resolved by the scene root's script= path).
        db.set_file_text(
            FileId(0),
            "func volume() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://music.gd");
        // music.tscn: a root Node with script=music.gd.
        db.set_file_text(
            FileId(1),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://music.gd\" id=\"1\"]\n\
             [node name=\"Music\" type=\"Node\"]\n\
             script = ExtResource(\"1\")\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://music.tscn");
        db.set_file_text(
            FileId(2),
            "func f():\n\tvar v := Music.volume()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://main.gd");
        db.set_project_config("[autoload]\nMusic=\"*res://music.tscn\"\n");
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("f unit");
        assert_eq!(
            unit.result.bindings[0].ty.label(api).as_deref(),
            Some("int"),
            "Music.volume() should resolve via the scene root's script",
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

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
    fn aliased_self_resolves_own_members_no_false_unsafe() {
        // `var me := self; me.own()` must resolve `own` via the file's OWN members — self is the
        // script's own class (a self-ScriptRef), not just its engine base. Before the fix `me` was
        // typed as the base (`Node`), so `me.own()` false-warned UNSAFE_METHOD_ACCESS.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "extends Node\nfunc own() -> int:\n\treturn 1\nfunc use_it():\n\tvar me := self\n\tvar n := me.own()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        // `me.own()` resolves to int (own member via aliased self) — proves it isn't the seam.
        assert!(
            fi.units
                .iter()
                .flat_map(|u| &u.result.bindings)
                .any(|b| b.ty.label(api).as_deref() == Some("int")),
            "aliased self.own() should resolve to int",
        );
        assert!(
            fi.diagnostics.is_empty(),
            "no false UNSAFE on aliased self: {:?}",
            fi.diagnostics
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

    // ---- M1: scene-aware node-path typing ($Path / %Unique) -------------------------------

    /// A db with file 0 = a scene and file 1 = its attached script, both with res:// paths.
    fn scene_db(scene_text: &str, gd_text: &str) -> RootDatabase {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), scene_text, Durability::LOW);
        db.set_file_path(FileId(0), "res://main.tscn");
        db.set_file_text(FileId(1), gd_text, Durability::LOW);
        db.set_file_path(FileId(1), "res://main.gd");
        db.sync_source_root();
        db
    }

    fn binding_labels(db: &RootDatabase) -> Vec<String> {
        let api = db.engine().unwrap();
        let fi = analyze_file(db, db.file_text(FileId(1)).unwrap());
        assert!(
            fi.diagnostics.is_empty(),
            "unexpected diags: {:?}",
            fi.diagnostics
        );
        fi.units
            .iter()
            .flat_map(|u| &u.result.bindings)
            .filter_map(|b| b.ty.label(api))
            .collect()
    }

    const SCENE: &str = "[gd_scene format=3]\n\
        [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
        [node name=\"Root\" type=\"Control\"]\n\
        script = ExtResource(\"1\")\n\
        [node name=\"Panel\" type=\"Panel\" parent=\".\"]\n\
        [node name=\"Box\" type=\"VBoxContainer\" parent=\"Panel\"]\n\
        [node name=\"Btn\" type=\"Button\" parent=\"Panel/Box\"]\n\
        unique_name_in_owner = true\n";

    #[test]
    fn dollar_path_types_to_the_concrete_node() {
        // `$Panel/Box/Btn` → Button (not bare Node) — the killer feature, zero annotations.
        let db = scene_db(
            SCENE,
            "extends Control\nfunc _ready():\n\tvar b := $Panel/Box/Btn\n",
        );
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "$Panel/Box/Btn should type as Button",
        );
    }

    #[test]
    fn unique_name_path_types_to_the_concrete_node() {
        // `%Btn` resolves via unique_name_in_owner → Button.
        let db = scene_db(SCENE, "extends Control\nfunc _ready():\n\tvar b := %Btn\n");
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "%Btn should type as Button"
        );
    }

    #[test]
    fn onready_var_from_a_node_path_is_typed() {
        // `@onready var x := $Path` types `x` from the resolved node at the decl site. (`:=` is the
        // typed form; plain `=` stays `Variant` per Godot's gradual typing — Phase-2 rule.)
        let db = scene_db(
            SCENE,
            "extends Control\n@onready var btn := $Panel/Box/Btn\n",
        );
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "@onready var := $Path should type to Button",
        );
    }

    #[test]
    fn get_node_string_literal_types_like_dollar() {
        // `get_node("Panel/Box/Btn")` (string literal) types identically to `$Panel/Box/Btn`.
        let db = scene_db(
            SCENE,
            "extends Control\nfunc _ready():\n\tvar b := get_node(\"Panel/Box/Btn\")\n",
        );
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "get_node(\"...\") should type as Button",
        );
    }

    #[test]
    fn self_get_node_string_literal_types_like_dollar() {
        // `self.get_node("…")` (explicit self = the attach node) types like the bare form; a foreign
        // receiver `obj.get_node("…")` stays a normal call → `Node` (can't resolve another node's path).
        let db = scene_db(
            SCENE,
            "extends Control\nfunc _ready():\n\tvar b := self.get_node(\"Panel/Box/Btn\")\n",
        );
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "self.get_node(\"...\") should type as Button",
        );
    }

    #[test]
    fn attached_script_refines_the_node_type() {
        // A node `type="Button"` + `script=Fancy.gd (class_name Fancy)` → `$That` is `Fancy`, so
        // `$That.fancy()` resolves to its cross-file return type (proving the script refine).
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [ext_resource type=\"Script\" path=\"res://fancy.gd\" id=\"2\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"That\" type=\"Button\" parent=\".\"]\n\
             script = ExtResource(\"2\")\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://main.tscn");
        db.set_file_text(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar n := $That.fancy()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.set_file_text(
            FileId(2),
            "class_name Fancy\nextends Button\nfunc fancy() -> int:\n\treturn 1\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://fancy.gd");
        db.sync_source_root();
        assert!(
            binding_labels(&db).iter().any(|l| l == "int"),
            "$That.fancy() should resolve via the attached script Fancy",
        );
    }

    #[test]
    fn computed_or_unresolvable_node_path_stays_node_without_warning() {
        // A computed `get_node(var)` and a `$Nope` with no owning scene both stay `Node` — never a
        // false node-path warning.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(1),
            "extends Node\nfunc f(p):\n\tvar a := get_node(p)\n\tvar b := $Nope\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://lone.gd");
        db.sync_source_root();
        let fi = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        assert!(
            fi.diagnostics.is_empty(),
            "no false node-path warnings: {:?}",
            fi.diagnostics
        );
    }

    // ---- M2: INVALID_NODE_PATH (the no-false-positive contract) ----------------------------

    fn has_invalid_node_path(db: &RootDatabase) -> bool {
        let fi = analyze_file(db, db.file_text(FileId(1)).unwrap());
        fi.diagnostics
            .iter()
            .any(|d| d.code == crate::infer::INVALID_NODE_PATH)
    }

    #[test]
    fn invalid_node_path_warns_when_genuinely_absent_in_a_single_owning_scene() {
        let db = scene_db(SCENE, "extends Control\nfunc _ready():\n\tvar b := $Nope\n");
        assert!(
            has_invalid_node_path(&db),
            "$Nope is absent in the one owning scene → warn"
        );
    }

    #[test]
    fn escape_and_absolute_paths_never_warn() {
        // `..` and absolute `/root/…` escape the scene slice — silent, never INVALID_NODE_PATH.
        let db = scene_db(
            SCENE,
            "extends Control\nfunc _ready():\n\tvar a := $\"../Sibling\"\n\tvar c := $\"/root/Global\"\n",
        );
        assert!(!has_invalid_node_path(&db), "escape paths must not warn");
    }

    #[test]
    fn path_descending_into_an_instanced_subscene_never_warns() {
        // Root > Player(instance=…). `$Player/Gun` misses below an instance we don't recurse into —
        // silent (the node may well exist inside the sub-scene).
        let db = scene_db(
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [ext_resource type=\"PackedScene\" path=\"res://player.tscn\" id=\"2\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Player\" parent=\".\" instance=ExtResource(\"2\")]\n",
            "extends Control\nfunc _ready():\n\tvar g := $Player/Gun\n",
        );
        assert!(
            !has_invalid_node_path(&db),
            "into-instance miss must not warn"
        );
    }

    #[test]
    fn ambiguous_multi_scene_attachment_suppresses_the_invalid_warning() {
        // main.gd attaches to BOTH a.tscn (child Alpha) and b.tscn (child Beta). `$Beta` is absent in
        // a.tscn (kept first) but present in b.tscn → ambiguous → no false INVALID_NODE_PATH.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Alpha\" type=\"Button\" parent=\".\"]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://a.tscn");
        db.set_file_text(
            FileId(2),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Beta\" type=\"Button\" parent=\".\"]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://b.tscn");
        db.set_file_text(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar b := $Beta\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.sync_source_root();
        assert!(
            !has_invalid_node_path(&db),
            "ambiguous multi-scene attachment must not warn"
        );
    }

    // ---- M3: instanced sub-scene recursion ------------------------------------------------

    #[test]
    fn instanced_node_recurses_into_the_subscene_root_script() {
        // main.tscn: Root(script=main.gd) > Enemy(instance=enemy.tscn). enemy.tscn's root carries
        // script=enemy.gd (class_name Enemy, `hp() -> int`). `$Enemy.hp()` must recurse into the
        // sub-scene root, refine to the Enemy script, and resolve the cross-file method → `int`
        // (proving the instance recursion + script refine; a bare `Node` would have no `hp()`).
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [ext_resource type=\"PackedScene\" path=\"res://enemy.tscn\" id=\"2\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Enemy\" parent=\".\" instance=ExtResource(\"2\")]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://main.tscn");
        db.set_file_text(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar e := $Enemy.hp()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.set_file_text(
            FileId(2),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://enemy.gd\" id=\"1\"]\n\
             [node name=\"Enemy\" type=\"Button\"]\n\
             script = ExtResource(\"1\")\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://enemy.tscn");
        db.set_file_text(
            FileId(3),
            "class_name Enemy\nextends Button\nfunc hp() -> int:\n\treturn 1\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(3), "res://enemy.gd");
        db.sync_source_root();
        assert!(
            binding_labels(&db).iter().any(|l| l == "int"),
            "$Enemy.hp() should recurse into the instanced sub-scene root's script Enemy",
        );
    }

    // ---- Phase-4 hunt fixes: `%`-segment paths (no false INVALID_NODE_PATH) ----------------

    #[test]
    fn unique_name_subpath_resolves_to_the_child_without_warning() {
        // `%Box/Btn`: resolve the unique `%Box`, then walk `/Btn` to its Button child — idiomatic
        // Godot. Must type as Button and NOT raise INVALID_NODE_PATH (the bare-map lookup of the
        // whole joined "Box/Btn" used to miss → false warning).
        let db = scene_db(
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Box\" type=\"VBoxContainer\" parent=\".\"]\n\
             unique_name_in_owner = true\n\
             [node name=\"Btn\" type=\"Button\" parent=\"Box\"]\n",
            "extends Control\nfunc _ready():\n\tvar b := %Box/Btn\n",
        );
        assert!(
            binding_labels(&db).iter().any(|l| l == "Button"),
            "%Box/Btn → Button (and no false INVALID_NODE_PATH)",
        );
    }

    #[test]
    fn percent_prefixed_string_paths_resolve_as_unique_without_warning() {
        // `get_node("%Btn")` and `$"%Btn"` are unique-name lookups (the `%` prefix lives inside the
        // string), NOT a child literally named "%Btn". Must type as Button with no INVALID_NODE_PATH.
        let db = scene_db(
            SCENE,
            "extends Control\nfunc _ready():\n\tvar a := get_node(\"%Btn\")\n\tvar b := $\"%Btn\"\n",
        );
        let labels = binding_labels(&db);
        assert!(
            labels.iter().filter(|l| *l == "Button").count() >= 2,
            "both %Btn string forms should resolve to Button: {labels:?}",
        );
    }
}
