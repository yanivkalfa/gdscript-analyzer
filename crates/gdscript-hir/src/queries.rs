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
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use gdscript_base::FileId;
use gdscript_scene::{NodeIdx, SceneModel};

use crate::infer::FileInference;
use crate::item_tree::{ItemTree, Member};
use crate::ty::{ScriptRefId, Ty};
use crate::warnings::{SuppressionMap, WarningSettings};

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
/// `FileId` order (the file set is sorted), so resolution is deterministic. Collision *diagnostics*
/// (warning at each duplicate declaration) are the separate [`class_name_collisions`] projection.
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

/// The set of global `class_name`s declared by **more than one** file in `root` — the shadowing
/// diagnostic's cross-file half. Mirrors [`global_registry`]'s firewall exactly: it reads only the
/// offset-free [`file_class_name`] projection of each file (never a body or byte range), so a
/// keystroke never rebuilds it. `global_registry` keeps the *first* declarer silently; this query
/// names the duplicates so [`crate::infer::analyze_file`] can warn at each colliding declaration.
#[salsa::tracked]
pub fn class_name_collisions(db: &dyn Db, root: SourceRoot) -> Arc<FxHashSet<SmolStr>> {
    let mut seen: FxHashSet<SmolStr> = FxHashSet::default();
    let mut dups: FxHashSet<SmolStr> = FxHashSet::default();
    for &file in root.files(db) {
        if let Some(name) = file_class_name(db, file)
            && !seen.insert(name.clone())
        {
            dups.insert(name);
        }
    }
    Arc::new(dups)
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
    /// `*`-flagged autoloads — the bare names that resolve as globals in code.
    singletons: FxHashMap<SmolStr, SmolStr>,
    /// Non-`*` autoloads — loaded at `/root/Name` but NOT global identifiers. Tracked so a
    /// `get_node("/root/Name")` access can still resolve them (a singleton lives there too).
    loaded: FxHashMap<SmolStr, SmolStr>,
}

impl AutoloadRegistry {
    /// The resource path of the **singleton** (`*`-flagged) autoload named `name`, if any — the only
    /// autoloads exposed as bare-name globals. A non-singleton name returns `None` here (it is not a
    /// global); use [`resolve_any_path`](AutoloadRegistry::resolve_any_path) for `/root/Name`.
    #[must_use]
    pub fn resolve_path(&self, name: &str) -> Option<&SmolStr> {
        self.singletons.get(name)
    }

    /// The resource path of **any** autoload named `name` — singleton or loaded-but-not-global. Both
    /// live at `/root/Name`, so this is what a `get_node("/root/Name")` access resolves through.
    #[must_use]
    pub fn resolve_any_path(&self, name: &str) -> Option<&SmolStr> {
        self.singletons.get(name).or_else(|| self.loaded.get(name))
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
    let mut loaded = FxHashMap::default();
    for e in crate::project::parse_autoloads(config.project_godot_text(db)) {
        if e.is_singleton {
            singletons.entry(e.name).or_insert(e.path);
        } else {
            loaded.entry(e.name).or_insert(e.path);
        }
    }
    Arc::new(AutoloadRegistry { singletons, loaded })
}

/// The Godot engine `(major, minor)` declared by `project.godot`'s `[application]`
/// `config/features`, or `None` if unspecified. Keyed on [`ProjectConfig`] alone (MEDIUM
/// durability), so it backdates across `.gd` body edits — the same cross-file firewall as
/// [`autoload_registry`].
///
/// Phase-5 plumbing: the value is exposed for engine-API-model selection, but only ONE engine model
/// is bundled today (`gdscript_api::GODOT_VERSION`), so it is currently informational. Phase 6
/// (multi-version bundling via the Godot-sync job) will use it to pick the matching `ApiInput`,
/// snapping to the nearest bundled minor and defaulting to the newest when absent.
#[salsa::tracked]
pub fn engine_version(db: &dyn Db, config: ProjectConfig) -> Option<(u32, u32)> {
    crate::project::parse_engine_version(config.project_godot_text(db))
}

/// Convenience over [`engine_version`]: the project's declared engine `(major, minor)`, or `None`
/// when there is no `project.godot` or it declares no version.
#[must_use]
pub fn project_engine_version(db: &dyn Db) -> Option<(u32, u32)> {
    engine_version(db, db.project_config()?)
}

/// The project's resolved warning settings, parsed from `project.godot`'s
/// `debug/gdscript/warnings/*` (Workstream 1). Keyed on [`ProjectConfig`] alone (MEDIUM
/// durability) and reads no `.gd` body, so **editing a warning level invalidates only this query +
/// the downstream gate, never `analyze_file`/`item_tree`/`infer`** — the salsa-cacheability
/// invariant the gating seam depends on (W1 §3.4/§6).
#[salsa::tracked]
pub fn warning_settings(db: &dyn Db, config: ProjectConfig) -> Arc<WarningSettings> {
    let text = config.project_godot_text(db);
    let engine =
        crate::project::parse_engine_version(text).unwrap_or_else(crate::warnings::bundled_version);
    Arc::new(crate::project::parse_warning_settings(text, engine))
}

/// The per-file `@warning_ignore[_start|_restore]` suppression map (Workstream 1). Keyed on the
/// file's parse — CST byte ranges are stable across incremental edits — so it recomputes only when
/// the file text changes, never on a warning-setting edit.
#[salsa::tracked]
pub fn suppression_map(db: &dyn Db, file: FileText) -> Arc<SuppressionMap> {
    Arc::new(crate::warnings::build_suppression_map(
        &parse(db, file).syntax_node(),
        file.text(db),
    ))
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
            // A `## @return-tuple(T0, T1, …)` doc-tag (BUG A3) wins over the plain annotation:
            // the method's call result is a `Ty::Tuple`, so a constant index on it projects the
            // element's real type cross-file too (`Hooks.useState(...)[1]` → `Callable`).
            Member::Func(f) => MemberSig::Method(f.tuple_return.as_ref().map_or_else(
                || resolve_ann(f.return_type.as_deref()),
                |names| {
                    Ty::Tuple(
                        names
                            .iter()
                            .map(|n| crate::resolve::resolve_type_name(db, api, n))
                            .collect(),
                    )
                },
            )),
            Member::Var(v) => MemberSig::Field(resolve_ann(v.type_ref.as_deref())),
            // `const X = preload("res://…")` (no annotation) resolves cross-file to the preloaded
            // script's `ScriptRef` (the SCRIPT meta-type) — the same resolution the declaring file does
            // same-file, which the offset-free projection otherwise drops. A relative path is anchored
            // to this file's dir. An explicit annotation wins.
            Member::Const(c) => MemberSig::Field(
                c.type_ref
                    .is_none()
                    .then_some(c.preload_path.as_deref())
                    .flatten()
                    .and_then(|raw| {
                        crate::resolve::anchor_res_path(file.res_path(db).as_deref(), raw)
                    })
                    .map_or_else(
                        || resolve_ann(c.type_ref.as_deref()),
                        |abs| {
                            crate::resolve::resolve_external(
                                db,
                                &crate::resolve::ExternalRef::Preload(abs),
                            )
                        },
                    ),
            ),
            Member::Signal(_) => MemberSig::Signal,
            // Enums + inner classes aren't modeled as instance members yet (M2+).
            Member::Enum(_) | Member::Class(_) => continue,
        };
        members.insert(SmolStr::new(name), sig);
    }
    // The resolved `extends` base — a user `ScriptRef` (another class_name / "res://…") walks
    // into the inheritance chain; an engine `Object` ends it at the API table.
    let base = crate::resolve::resolve_base(db, api, &tree, file.res_path(db).as_deref());
    Arc::new(ScriptClass { members, base })
}

// ---- M1: scenes (.tscn/.tres) ------------------------------------------------------------

/// Whether a `res://` path is a *text* scene/resource we parse (`.tscn`/`.tres`). Binary
/// `.scn`/`.res` are detected-and-degraded by the parser, but we don't waste a parse on a `.gd`.
#[must_use]
pub fn is_scene_path(path: &str) -> bool {
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

/// **Every** scene that attaches the script at `res_path` — `(scene file, attach node)` pairs in
/// scan order. Single-scene scripts have one entry; the rare multi-scene script has several (the
/// basis for `$Path` **union typing**, M2 §6.3 — type a path as the common base across all scenes).
/// Same firewall as [`script_scene_index`] (keyed on the scene texts, backdates across `.gd` edits).
#[salsa::tracked]
pub fn script_scene_attachments(
    db: &dyn Db,
    root: SourceRoot,
) -> Arc<FxHashMap<SmolStr, Vec<(FileId, NodeIdx)>>> {
    let mut map: FxHashMap<SmolStr, Vec<(FileId, NodeIdx)>> = FxHashMap::default();
    for &file in root.files(db) {
        if !file.res_path(db).as_deref().is_some_and(is_scene_path) {
            continue;
        }
        let model = scene_model(db, file);
        let scene = file.file_id(db);
        for (i, node) in model.nodes.iter().enumerate() {
            let Some(path) = node
                .script
                .as_ref()
                .and_then(|id| model.ext_resources.get(id))
                .and_then(|e| e.path.clone())
            else {
                continue;
            };
            map.entry(path)
                .or_default()
                .push((scene, NodeIdx(u32::try_from(i).unwrap_or(u32::MAX))));
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

    // ---- W2: class_name collision / shadowing diagnostics ---------------------------------

    use crate::infer::SHADOWED_GLOBAL_IDENTIFIER;

    fn shadow_codes(fi: &Arc<FileInference>) -> Vec<&str> {
        fi.diagnostics
            .iter()
            .filter(|d| d.code == SHADOWED_GLOBAL_IDENTIFIER)
            .map(|d| d.code.as_str())
            .collect()
    }

    #[test]
    fn class_name_collisions_names_only_the_duplicates() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "class_name Dup\n", Durability::LOW);
        db.set_file_text(FileId(1), "class_name Dup\n", Durability::LOW);
        db.set_file_text(FileId(2), "class_name Unique\n", Durability::LOW);
        db.sync_source_root();
        let root = db.source_root().unwrap();

        let cols = class_name_collisions(&db, root);
        assert!(cols.contains(&SmolStr::new("Dup")));
        assert!(
            !cols.contains(&SmolStr::new("Unique")),
            "a singly-declared class_name is not a collision",
        );
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn missing_tool_warns_when_extending_a_tool_base_without_tool() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "@tool\nclass_name ToolBase\n", Durability::LOW);
        db.set_file_text(
            FileId(1),
            "extends ToolBase\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let derived = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        assert!(
            derived
                .raw_warnings
                .iter()
                .any(|w| w.code.as_str() == "MISSING_TOOL"),
            "{:?}",
            derived
                .raw_warnings
                .iter()
                .map(|w| w.code.as_str())
                .collect::<Vec<_>>()
        );
        // The base itself IS `@tool` → no warning.
        let base = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        assert!(
            !base
                .raw_warnings
                .iter()
                .any(|w| w.code.as_str() == "MISSING_TOOL")
        );
    }

    #[test]
    fn a_tool_class_extending_a_tool_base_is_silent() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "@tool\nclass_name ToolBase\n", Durability::LOW);
        db.set_file_text(FileId(1), "@tool\nextends ToolBase\n", Durability::LOW);
        db.sync_source_root();
        let derived = analyze_file(&db, db.file_text(FileId(1)).unwrap());
        assert!(
            !derived
                .raw_warnings
                .iter()
                .any(|w| w.code.as_str() == "MISSING_TOOL")
        );
    }

    #[test]
    fn duplicate_class_name_warns_at_both_declarations() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Dup\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.set_file_text(FileId(1), "class_name Dup\nvar x := 1\n", Durability::LOW);
        db.sync_source_root();

        for fid in [0, 1] {
            let fi = analyze_file(&db, db.file_text(FileId(fid)).unwrap());
            assert!(
                shadow_codes(&fi).contains(&SHADOWED_GLOBAL_IDENTIFIER),
                "file {fid} should warn on the duplicate class_name: {:?}",
                fi.diagnostics
            );
            // The warning points at the NAME (`Dup` at offset 11), not byte 0 or the keyword.
            let d = fi
                .diagnostics
                .iter()
                .find(|d| d.code == SHADOWED_GLOBAL_IDENTIFIER)
                .unwrap();
            assert_eq!(d.range, gdscript_base::TextRange::new(11, 14));
        }
    }

    #[test]
    fn class_name_shadowing_an_engine_class_warns() {
        let mut db = RootDatabase::default();
        // `Node` is an engine class — declaring `class_name Node` shadows it.
        db.set_file_text(
            FileId(0),
            "class_name Node\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        assert!(
            shadow_codes(&fi).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "class_name Node must warn (shadows the engine class): {:?}",
            fi.diagnostics
        );
    }

    #[test]
    fn class_name_shadowing_a_builtin_type_warns() {
        let mut db = RootDatabase::default();
        // `Vector2` is a builtin Variant type — a `class_name Vector2` hides it.
        db.set_file_text(FileId(0), "class_name Vector2\n", Durability::LOW);
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        assert!(
            shadow_codes(&fi).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "{:?}",
            fi.diagnostics
        );
    }

    #[test]
    fn class_name_shadowing_a_star_autoload_warns() {
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name Game\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://game.gd");
        // A `*`-singleton named `Game` — the class_name now hides the autoload global.
        db.set_project_config("[autoload]\nGame=\"*res://other.gd\"\n");
        db.sync_source_root();

        let fi = analyze_file(&db, db.file_text(FileId(0)).unwrap());
        assert!(
            shadow_codes(&fi).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "class_name Game must warn (shadows the `*Game` autoload): {:?}",
            fi.diagnostics
        );
    }

    #[test]
    fn unique_non_shadowing_class_name_does_not_warn() {
        // No false positive: a one-of-a-kind name that is no engine/builtin/autoload symbol.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name MyVeryOwnUniquePlayer\nfunc f():\n\tpass\n",
            Durability::LOW,
        );
        db.set_file_text(
            FileId(1),
            "class_name AnotherUniqueEnemy\n",
            Durability::LOW,
        );
        db.sync_source_root();

        for fid in [0, 1] {
            let fi = analyze_file(&db, db.file_text(FileId(fid)).unwrap());
            assert!(
                shadow_codes(&fi).is_empty(),
                "file {fid}: a unique class_name must not warn: {:?}",
                fi.diagnostics
            );
        }
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
    fn cyclic_extends_flags_each_cycle_member_and_terminates() {
        use crate::infer::CYCLIC_INHERITANCE;
        let mut db = RootDatabase::default();
        // A extends B extends A — illegal in Godot. The member walk must not loop, AND each file on
        // the cycle must be flagged `CYCLIC_INHERITANCE` at its own `extends` decl.
        db.set_file_text(FileId(0), "class_name A\nextends B\n", Durability::LOW);
        db.set_file_text(FileId(1), "class_name B\nextends A\n", Durability::LOW);
        // A third, ACYCLIC file that merely USES `A` — it is not on the cycle, so it must stay clean.
        db.set_file_text(
            FileId(2),
            "func use_it():\n\tvar a: A\n\tvar x := a.nope()\n",
            Durability::LOW,
        );
        db.sync_source_root();

        // Each cycle member is flagged exactly once, at its own `extends`.
        for id in [FileId(0), FileId(1)] {
            let fi = analyze_file(&db, db.file_text(id).unwrap());
            let cyclic: Vec<_> = fi
                .diagnostics
                .iter()
                .filter(|d| d.code == CYCLIC_INHERITANCE)
                .collect();
            assert_eq!(cyclic.len(), 1, "file {id:?}: {:?}", fi.diagnostics);
        }

        // The user file is off the cycle — `a.nope()` walks A->B->A->… and bottoms out at the seam
        // (no panic/hang, no diagnostic on this file).
        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn cyclic_extends_via_res_path_two_files_flags_no_hang() {
        use crate::infer::CYCLIC_INHERITANCE;
        let mut db = RootDatabase::default();
        // a.gd extends "res://b.gd"; b.gd extends "res://a.gd" — a 2-file `res://` path cycle.
        set_with_path(&mut db, 0, "res://a.gd", "extends \"res://b.gd\"\n");
        set_with_path(&mut db, 1, "res://b.gd", "extends \"res://a.gd\"\n");
        db.sync_source_root();

        for id in [FileId(0), FileId(1)] {
            let fi = analyze_file(&db, db.file_text(id).unwrap());
            assert!(
                fi.diagnostics.iter().any(|d| d.code == CYCLIC_INHERITANCE),
                "file {id:?} expected CYCLIC_INHERITANCE: {:?}",
                fi.diagnostics
            );
        }
    }

    #[test]
    fn deep_acyclic_extends_chain_does_not_false_fire() {
        use crate::infer::CYCLIC_INHERITANCE;
        let mut db = RootDatabase::default();
        // A 5-deep ACYCLIC chain bottoming out at an engine base: C0 -> C1 -> ... -> C4 -> Node.
        // None revisits the start, so NONE may be flagged `CYCLIC_INHERITANCE`.
        db.set_file_text(FileId(0), "class_name C0\nextends C1\n", Durability::LOW);
        db.set_file_text(FileId(1), "class_name C1\nextends C2\n", Durability::LOW);
        db.set_file_text(FileId(2), "class_name C2\nextends C3\n", Durability::LOW);
        db.set_file_text(FileId(3), "class_name C3\nextends C4\n", Durability::LOW);
        db.set_file_text(FileId(4), "class_name C4\nextends Node\n", Durability::LOW);
        db.sync_source_root();

        for id in (0..5).map(FileId) {
            let fi = analyze_file(&db, db.file_text(id).unwrap());
            assert!(
                !fi.diagnostics.iter().any(|d| d.code == CYCLIC_INHERITANCE),
                "file {id:?} false-fired CYCLIC_INHERITANCE: {:?}",
                fi.diagnostics
            );
        }
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
    fn cross_file_preload_const_member_resolves() {
        // The xfile-preload-const fix: another file reading `Holder.W` where
        // `const W = preload("res://widget.gd")` resolves W to the preloaded script. Previously the
        // offset-free script_class projection saw only the const's (absent) annotation → Variant.
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
            "res://holder.gd",
            "class_name Holder\nconst W = preload(\"res://widget.gd\")\n",
        );
        set_with_path(
            &mut db,
            2,
            "res://user.gd",
            "func use_it():\n\tvar a := Holder.W.make()\n",
        );
        db.sync_source_root();
        let api = db.engine().unwrap();

        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| !u.result.bindings.is_empty())
            .expect("use_it unit");
        // Holder.W → Widget's ScriptRef → .make() → int (cross-file, through the const).
        assert_eq!(
            unit.result.bindings[0].ty.label(api).as_deref(),
            Some("int"),
            "Holder.W.make() should resolve cross-file to int, got {:?}",
            unit.result.bindings[0].ty
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
    fn relative_extends_path_anchors_to_importing_dir() {
        let mut db = RootDatabase::default();
        // base.gd under entities/, reachable only by path (no class_name).
        set_with_path(
            &mut db,
            0,
            "res://entities/base.gd",
            "extends Node\nfunc base_method() -> int:\n\treturn 1\n",
        );
        // derived.gd in the SAME dir uses a RELATIVE `extends "base.gd"` (anchored to entities/).
        set_with_path(
            &mut db,
            1,
            "res://entities/derived.gd",
            "class_name Derived\nextends \"base.gd\"\nfunc own() -> String:\n\treturn \"x\"\n",
        );
        set_with_path(
            &mut db,
            2,
            "res://main.gd",
            "func use_it():\n\tvar d: Derived\n\tvar a := d.own()\n\tvar b := d.base_method()\n",
        );
        db.sync_source_root();
        let api = db.engine().unwrap();
        let fi = analyze_file(&db, db.file_text(FileId(2)).unwrap());
        let unit = fi
            .units
            .iter()
            .find(|u| u.result.bindings.len() >= 3)
            .expect("use_it unit with 3 bindings (d, a, b)");
        // bindings: [0]=`d: Derived`, [1]=own() (own member), [2]=base_method() (relative-extends base).
        assert_eq!(
            unit.result.bindings[1].ty.label(api).as_deref(),
            Some("String")
        );
        assert_eq!(
            unit.result.bindings[2].ty.label(api).as_deref(),
            Some("int"),
            "base_method() must resolve through the relative `extends \"base.gd\"`"
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
    fn star_autoload_scene_resolves_via_script_class_shortcut() {
        // A `*`-autoload `.tscn` whose root has NO `script=` ext_resource but carries the header
        // `script_class="…"` shortcut resolves through the class_name registry (the recorded
        // shortcut, without a script ext_resource). Autoload name `Audio` ≠ class_name `MusicPlayer`
        // so the resolution can ONLY go via the scene's script_class shortcut.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class_name MusicPlayer\nfunc volume() -> int:\n\treturn 5\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://music.gd");
        db.set_file_text(
            FileId(1),
            "[gd_scene format=3 script_class=\"MusicPlayer\"]\n[node name=\"Root\" type=\"Node\"]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://music.tscn");
        db.set_file_text(
            FileId(2),
            "func f():\n\tvar v := Audio.volume()\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://main.gd");
        db.set_project_config("[autoload]\nAudio=\"*res://music.tscn\"\n");
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
            "Audio.volume() should resolve via the scene's script_class= shortcut",
        );
        assert!(fi.diagnostics.is_empty(), "diags: {:?}", fi.diagnostics);
    }

    #[test]
    fn engine_version_from_project_config_is_firewalled_against_body_edits() {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), "func f():\n\tpass\n", Durability::LOW);
        db.set_file_path(FileId(0), "res://main.gd");
        db.set_project_config("[application]\nconfig/features=PackedStringArray(\"4.6\")\n");
        db.sync_source_root();
        assert_eq!(project_engine_version(&db), Some((4, 6)));

        // A `.gd` body edit must NOT change the project's declared engine version (the query is
        // keyed on ProjectConfig alone — the cross-file firewall).
        db.set_file_text(FileId(0), "func f():\n\tvar x := 1\n", Durability::LOW);
        db.sync_source_root();
        assert_eq!(project_engine_version(&db), Some((4, 6)));

        // No `project.godot` → no declared version.
        let empty = RootDatabase::default();
        assert_eq!(project_engine_version(&empty), None);
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

    // The W1 gating firewall (the M0 load-bearing test): editing a `debug/gdscript/warnings/*`
    // level must re-run only `warning_settings` (+ the downstream gate in `type_diagnostics`),
    // NEVER the cached `analyze_file` (inference). Severity is resolved downstream, so a settings
    // edit leaves inference's `raw_warnings` untouched.

    static ANALYZE_OBSERVED: AtomicU32 = AtomicU32::new(0);

    #[salsa::tracked]
    fn observe_analyze_file(db: &dyn gdscript_db::Db, file: FileText) -> usize {
        ANALYZE_OBSERVED.fetch_add(1, Ordering::SeqCst);
        analyze_file(db, file).raw_warnings.len()
    }

    #[test]
    fn warning_level_edit_does_not_invalidate_analyze_file() {
        use crate::warnings::{WarnLevel, WarningCode};

        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "func f():\n\tvar x = 5 / 2\n\treturn x\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://game.gd");
        db.set_project_config(
            "[autoload]\nGame=\"*res://game.gd\"\n[debug]\ngdscript/warnings/integer_division=2\n",
        );
        db.sync_source_root();
        let file = db.file_text(FileId(0)).unwrap();
        let config = db.project_config().unwrap();

        // Prime: analyze_file runs once and records the gateable raw warnings — INTEGER_DIVISION
        // plus UNTYPED_DECLARATION (the untyped `var x`).
        assert_eq!(observe_analyze_file(&db, file), 2);
        let runs = ANALYZE_OBSERVED.load(Ordering::SeqCst);
        assert_eq!(
            warning_settings(&db, config)
                .per_code
                .get(&WarningCode::IntegerDivision),
            Some(&WarnLevel::Error),
        );

        // Edit ONLY the warning level (the `[autoload]` line is byte-identical). analyze_file must
        // not recompute — its inputs (file text, engine, the autoload registry's *value*) are
        // unchanged; only `warning_settings` re-runs.
        db.set_project_config(
            "[autoload]\nGame=\"*res://game.gd\"\n[debug]\ngdscript/warnings/integer_division=1\n",
        );
        assert_eq!(observe_analyze_file(&db, file), 2);
        assert_eq!(
            ANALYZE_OBSERVED.load(Ordering::SeqCst),
            runs,
            "REGRESSION: a warning-level edit re-ran analyze_file — the W1 gating firewall broke",
        );
        // The setting itself DID change (the test is not vacuous): the gate now sees WARN.
        assert_eq!(
            warning_settings(&db, config)
                .per_code
                .get(&WarningCode::IntegerDivision),
            Some(&WarnLevel::Warn),
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
            // `p: NodePath` so the computed `get_node(p)` still exercises node-path resolution but
            // without an (orthogonal, legitimate) UNSAFE_CALL_ARGUMENT on an untyped Variant arg —
            // that warning has its own tests in `infer`.
            "extends Node\nfunc f(p: NodePath):\n\tvar a := get_node(p)\n\tvar b := $Nope\n",
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

    #[test]
    fn path_into_an_instanced_subscene_types_the_inner_node() {
        // main.tscn: Root(script=main.gd) > Enemy(instance=enemy.tscn). enemy.tscn: Enemy(Node2D) >
        // Sprite(Sprite2D). M3 typed `$Enemy` itself; this §4b step continues the walk INTO the
        // sub-scene, so `$Enemy/Sprite` types as `Sprite2D` (the inner node), not bare `Node`.
        // `$Enemy/Nope` stays `Node` with NO false INVALID_NODE_PATH (binding_labels asserts zero
        // diagnostics).
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
            "extends Control\nfunc _ready():\n\tvar s := $Enemy/Sprite\n\tvar n := $Enemy/Nope\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.set_file_text(
            FileId(2),
            "[gd_scene format=3]\n\
             [node name=\"Enemy\" type=\"Node2D\"]\n\
             [node name=\"Sprite\" type=\"Sprite2D\" parent=\".\"]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://enemy.tscn");
        db.sync_source_root();
        assert!(
            binding_labels(&db).iter().any(|l| l == "Sprite2D"),
            "$Enemy/Sprite should type as the sub-scene's Sprite (Sprite2D)",
        );
    }

    #[test]
    fn override_child_under_an_instance_types_from_the_subscene() {
        // The "hard tail" (burndown Stage 4.23): main.tscn instances enemy.tscn at `Enemy` AND adds
        // an OVERRIDE child `[node name="Sprite" parent="Enemy"]` — a node with no own
        // `type=`/script/instance (it only carries property overrides). It RESOLVES to that outer
        // node (not IntoInstance), which used to floor to bare `Node`; now its type is taken from the
        // same-pathed node inside the instanced sub-scene → `Sprite2D`.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "[gd_scene format=3]\n\
             [ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\
             [ext_resource type=\"PackedScene\" path=\"res://enemy.tscn\" id=\"2\"]\n\
             [node name=\"Root\" type=\"Control\"]\n\
             script = ExtResource(\"1\")\n\
             [node name=\"Enemy\" parent=\".\" instance=ExtResource(\"2\")]\n\
             [node name=\"Sprite\" parent=\"Enemy\"]\n\
             modulate = Color(1, 0, 0, 1)\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(0), "res://main.tscn");
        db.set_file_text(
            FileId(1),
            "extends Control\nfunc _ready():\n\tvar s := $Enemy/Sprite\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(1), "res://main.gd");
        db.set_file_text(
            FileId(2),
            "[gd_scene format=3]\n\
             [node name=\"Enemy\" type=\"Node2D\"]\n\
             [node name=\"Sprite\" type=\"Sprite2D\" parent=\".\"]\n",
            Durability::LOW,
        );
        db.set_file_path(FileId(2), "res://enemy.tscn");
        db.sync_source_root();
        assert!(
            binding_labels(&db).iter().any(|l| l == "Sprite2D"),
            "an override child under an instance types from the sub-scene (Sprite2D), got: {:?}",
            binding_labels(&db),
        );
    }

    #[test]
    fn inner_class_value_and_members_type_via_its_item_tree() {
        // Burndown Stage 4.24 inc.1: an inner `class Inner:` value/instance types instead of seaming
        // (`Ty::InnerClass`, was `Ty::Unknown`). `Inner.new()` constructs an instance; member access
        // resolves against the inner class's own item-tree (by annotation) + its `extends` chain — so
        // `x.hp`/`x.ping()` type as `int` (no false `INFERENCE_ON_VARIANT`), and a member inherited
        // from the inner class's engine base (`extends Node` → `Object.get_class`) resolves to `String`.
        let mut db = RootDatabase::default();
        db.set_file_text(
            FileId(0),
            "class Inner extends Node:\n\
             \tvar hp: int = 5\n\
             \tfunc ping() -> int:\n\
             \t\treturn 1\n\
             \tfunc combo() -> int:\n\
             \t\tvar s := self.get_class()\n\
             \t\treturn hp + ping()\n\
             func f():\n\
             \tvar x := Inner.new()\n\
             \tvar a := x.hp\n\
             \tvar b := x.ping()\n\
             \tvar c := x.get_class()\n",
            Durability::LOW,
        );
        db.sync_source_root();
        let ft = db.file_text(FileId(0)).unwrap();
        let fi = analyze_file(&db, ft);
        assert!(
            fi.diagnostics.is_empty(),
            "no hard diagnostics expected: {:?}",
            fi.diagnostics
        );
        let api = db.engine().unwrap();
        let labels: Vec<String> = fi
            .units
            .iter()
            .flat_map(|u| &u.result.bindings)
            .filter_map(|b| b.ty.label(api))
            .collect();
        assert!(
            labels.iter().filter(|l| *l == "int").count() >= 2,
            "x.hp and x.ping() should both type as int: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == "String"),
            "x.get_class() should resolve via the inner class's `extends Node` chain to String: {labels:?}"
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
