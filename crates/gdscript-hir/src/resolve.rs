//! Name & type resolution (Playbook §3.2/§3.5): the [`resolve_external`] Phase-3 seam, the
//! GDScript source-annotation → [`Ty`] resolver, base-class resolution, the per-class
//! [`ClassScope`] (the class-member tier of the binder), and global resolution.
//!
//! The binder's lookup order (local → class member → inherited → global) is *driven* by
//! [`crate::infer`]; this module supplies the class-member and global tiers plus the type
//! resolution all tiers share. Everything here is a pure function of the item tree + the
//! `Arc`-shared [`EngineApi`] — no body, no cross-file state.

use cstree::util::NodeOrToken;
use gdscript_api::gdscript_layer::LayerTy;
use gdscript_api::{BuiltinId, ClassId, EngineApi};
use gdscript_db::Db;
use gdscript_syntax::{GdNode, SyntaxKind};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::item_tree::{ExtendsRef, ItemTree, Member};
use crate::ty::{EnumRef, ScriptRefId, Ty};

/// A reference that *would* require another file to resolve — the Phase-3 boundary. Phase 2
/// never reaches across files, so every variant resolves to the same non-cascading
/// [`Ty::Unknown`]; Phase 3 reimplements only [`resolve_external`], leaving every inference
/// body unchanged (Playbook §0 — "the biggest enabler in the whole phase; protect it").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalRef {
    /// A `class_name`-registered global from another script.
    ClassName(SmolStr),
    /// An `extends "res://…"` / `extends Other.Inner` target.
    ExtendsPath(SmolStr),
    /// A `preload(...)`/`load(...)` resource.
    Preload(SmolStr),
    /// A project autoload singleton.
    Autoload(SmolStr),
}

/// **The Phase-3 seam.** Resolve a cross-file reference. In Phase 2 this is *always*
/// [`Ty::Unknown`] — a type that never warns, never cascades a diagnostic, and is elided from
/// hover. Funnel every "would need another file" path through here so Phase 3 has exactly one
/// function to reimplement.
#[must_use]
pub fn resolve_external(db: &dyn Db, r: &ExternalRef) -> Ty {
    match r {
        // M1: a project-global `class_name` → its script reference.
        ExternalRef::ClassName(name) => resolve_class_name(db, name),
        // M3: `preload("res://x.gd")` → the declaring file's `ScriptRef` (a compile-time constant
        // SCRIPT meta-type in Godot; `reduce_preload` — resolved by `res://` PATH, independent of
        // `class_name`, so a script with no `class_name` is still preloadable). We reuse the
        // `ScriptRef` representation: `X.new()` → instance, `X.member`/`X.CONST` resolve via the
        // same `script_member_walk` as a `class_name` reference (the analyzer already collapses
        // the meta-vs-instance distinction, like a bare `class_name`).
        ExternalRef::Preload(path) => resolve_res_path(db, path),
        // M3: `extends "res://x.gd"` lights up the same path map. A *relative* / dotted form
        // (`extends "sibling.gd"`, `extends A.B`) stays the seam — relative-path anchoring is a
        // documented follow-up (needs the importing file's dir; 0 occurrences in the corpus).
        ExternalRef::ExtendsPath(path) if is_resource_path(path) => resolve_res_path(db, path),
        // M4: a `*`-flagged autoload singleton's bare name → its script `ScriptRef` — a `.gd`
        // directly, or a `.tscn` via its root node's attached script (Phase-4 scene-root sharpening).
        ExternalRef::Autoload(name) => resolve_autoload(db, name),
        // `load(...)` is never routed here (it stays an opaque runtime call). Dotted `extends`
        // remains the seam.
        ExternalRef::ExtendsPath(_) => Ty::Unknown,
    }
}

/// Resolve a `*`-singleton autoload's bare name (M4). A `.gd` autoload resolves by **path** to its
/// declaring file's [`Ty::ScriptRef`] (so `.member`/`.new()` walk via the script member table,
/// even when the script has no `class_name`). A scene (`.tscn`/`.scn`) or any other resource
/// autoload stays the **seam** ([`Ty::Unknown`]): typing it as bare `Node` would *false-warn* on
/// the scene root script's own members (e.g. `Music.play()`), which we cannot see until Phase 4
/// scene parsing recovers the root's real type — the conservative seam keeps zero false positives.
/// No project config, a non-singleton name, or a dangling path is likewise the seam.
fn resolve_autoload(db: &dyn Db, name: &str) -> Ty {
    let Some(config) = db.project_config() else {
        return Ty::Unknown;
    };
    let Some(path) = crate::queries::autoload_registry(db, config)
        .resolve_path(name)
        .cloned()
    else {
        return Ty::Unknown;
    };
    if is_gdscript_path(&path) {
        resolve_res_path(db, &path)
    } else if is_scene_path(&path) {
        resolve_scene_autoload(db, &path)
    } else {
        Ty::Unknown
    }
}

/// A `*`-autoload pointing at a scene (`.tscn`/`.tres`) resolves to its **root node's attached
/// script** — the singleton-scene pattern (`Music="*res://music.tscn"` whose root has
/// `script=music.gd`), so `Music.play()` checks against the real script (Phase-4 unblocked this; the
/// scene model is now ingested). A root with no script, or an un-loaded scene, → the conservative
/// seam. (Typing a script-less root by its native `type=` would need the engine API, which
/// `resolve_external` doesn't carry — a follow-up; the attached-script case is the common one.)
fn resolve_scene_autoload(db: &dyn Db, scene_path: &str) -> Ty {
    let Some(root) = db.source_root() else {
        return Ty::Unknown;
    };
    let Some(&scene_file) = crate::queries::res_path_registry(db, root).get(scene_path) else {
        return Ty::Unknown; // the scene isn't loaded into the VFS
    };
    let Some(ft) = db.file_text(scene_file) else {
        return Ty::Unknown;
    };
    let scene = crate::queries::scene_model(db, ft);
    // 1. An attached script on the root (`script=ExtResource`) — the most specific (a `.tscn`).
    if let Some(script_path) = scene
        .root
        .and_then(|idx| scene.node(idx))
        .and_then(|root_node| root_node.script.as_ref())
        .and_then(|id| scene.ext_resources.get(id))
        .and_then(|ext| ext.path.as_deref())
    {
        let ty = resolve_res_path(db, script_path);
        if !ty.is_uninformative() {
            return ty;
        }
    }
    // 2. The `.tscn` header `script_class="…"` shortcut, or a `.tres`'s own `resource_type` — the
    //    resource's `class_name`, recorded without resolving the script file (so a script-less root
    //    that still carries its class_name resolves). Resolve it through the project class_name
    //    registry. (Typing a root by its native `type=` alone would need the engine API, which this
    //    seam doesn't carry — a follow-up; resolving the recorded class_name is the common case.)
    for class_name in [scene.script_class.as_ref(), scene.resource_type.as_ref()]
        .into_iter()
        .flatten()
    {
        let ty = resolve_external(db, &ExternalRef::ClassName(class_name.clone()));
        if !ty.is_uninformative() {
            return ty;
        }
    }
    Ty::Unknown
}

/// Whether a resource path is a Godot scene/resource (`.tscn`/`.tres`).
fn is_scene_path(p: &str) -> bool {
    p.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tscn") || ext.eq_ignore_ascii_case("tres"))
}

/// Whether a resource path is a GDScript file (the `.cs` C# case is out of scope → seam). Compare
/// the final extension rather than `ends_with` so a `.GD` (case quirk) still matches.
fn is_gdscript_path(p: &str) -> bool {
    p.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gd"))
}

/// Whether a path is an engine resource URI we resolve project-root-absolutely (no anchor
/// needed). Godot also accepts relative `preload`/`extends` paths anchored to the importing
/// script's directory; those are a documented follow-up (they need the importing file's path
/// threaded into resolution, and the reference corpus has none).
fn is_resource_path(p: &str) -> bool {
    p.starts_with("res://") || p.starts_with("user://")
}

/// Anchor a `preload`/`extends` resource path to an absolute `res://`/`user://` path the way Godot
/// does (`reduce_preload`: `script_path.get_base_dir().path_join(p).simplify_path()`): an already-
/// absolute path passes through unchanged; a RELATIVE path is joined to `importing`'s directory and
/// simplified (`.`/`..` collapsed). `None` only when the path is relative and `importing` carries no
/// resource anchor — the conservative seam (never a false resolution).
#[must_use]
pub fn anchor_res_path(importing: Option<&str>, raw: &str) -> Option<SmolStr> {
    if is_resource_path(raw) {
        return Some(SmolStr::new(raw));
    }
    let (scheme, rest) = importing?.split_once("://")?;
    let dir = rest.rsplit_once('/').map_or("", |(d, _)| d);
    let joined = if dir.is_empty() {
        format!("{scheme}://{raw}")
    } else {
        format!("{scheme}://{dir}/{raw}")
    };
    Some(SmolStr::new(simplify_resource_path(&joined)))
}

/// Collapse `.`/`..`/empty segments in a `scheme://…` resource path (Godot's `simplify_path`).
fn simplify_resource_path(path: &str) -> String {
    let (scheme, rest) = path.split_once("://").unwrap_or(("res", path));
    let mut out: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("{scheme}://{}", out.join("/"))
}

/// Resolve a `res://` resource path to the declaring file's [`Ty::ScriptRef`] via the project
/// [`res_path_registry`](crate::queries::res_path_registry), or the seam ([`Ty::Unknown`]) when
/// no project is loaded or the path maps to no known file (a dangling `preload` — imprecise, but
/// never a false diagnostic).
fn resolve_res_path(db: &dyn Db, path: &str) -> Ty {
    // Only a GDScript resource has a script `ScriptRef`. A `.tscn`/`.tres`/`.png`/… resolves to a
    // PackedScene/Resource, not a script — typing it as a `ScriptRef` would wrongly accept
    // `X.new()` and member access on it (scene-root typing is Phase 4). The `res_path_registry`
    // only indexes `.gd` files today, but gate defensively so a future scene-ingesting loader
    // cannot mis-type `preload("res://x.tscn")`. Non-`.gd` → the conservative seam.
    if !is_gdscript_path(path) {
        return Ty::Unknown;
    }
    let Some(root) = db.source_root() else {
        return Ty::Unknown;
    };
    match crate::queries::res_path_registry(db, root).get(path) {
        Some(file) => Ty::ScriptRef(ScriptRefId(file.0)),
        None => Ty::Unknown,
    }
}

/// Resolve a global `class_name` against the project registry (M1): the script's
/// [`Ty::ScriptRef`], or the seam ([`Ty::Unknown`]) when no project is loaded or the name is not
/// a registered global class. The `ScriptRefId` is the declaring file's `FileId`.
fn resolve_class_name(db: &dyn Db, name: &str) -> Ty {
    let Some(root) = db.source_root() else {
        return Ty::Unknown;
    };
    match crate::queries::global_registry(db, root).resolve(name) {
        Some(file) => Ty::ScriptRef(ScriptRefId(file.file_id(db).0)),
        None => Ty::Unknown,
    }
}

// ---- type-annotation resolution ----------------------------------------------------------

/// Resolve a GDScript source type annotation (a `TypeRef` CST node) to a [`Ty`]. Handles
/// `void`/`Variant`, builtins, engine classes, `Array`/`Array[T]`, `Dictionary`/
/// `Dictionary[K, V]`, global enums, and `Class.Enum`; an unknown bare name is treated as a
/// (cross-file) `class_name` and funneled through the [`resolve_external`] seam.
#[must_use]
pub fn resolve_type_ref(db: &dyn Db, api: &EngineApi, node: &GdNode) -> Ty {
    // The leading dotted name comes from this node's *direct* `Ident`/`void` tokens; the type
    // arguments (`[...]`) are *direct child* `TypeRef` nodes (the grammar nests them).
    let names: Vec<String> = node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::VoidKw))
        .map(|t| t.text().to_owned())
        .collect();
    let args: Vec<GdNode> = node
        .children()
        .filter(|c| c.kind() == SyntaxKind::TypeRef)
        .cloned()
        .collect();
    resolve_named(db, api, &names, &args)
}

/// Resolve a bare type *name* (no type arguments) — for callers that only have a string
/// (completion detail, inlay display).
#[must_use]
pub fn resolve_type_name(db: &dyn Db, api: &EngineApi, name: &str) -> Ty {
    resolve_named(db, api, std::slice::from_ref(&name.to_owned()), &[])
}

fn resolve_named(db: &dyn Db, api: &EngineApi, names: &[String], args: &[GdNode]) -> Ty {
    let Some(head) = names.first() else {
        return Ty::Variant;
    };
    if names.len() == 1 {
        match head.as_str() {
            "void" => return Ty::Void,
            "Variant" => return Ty::Variant,
            // Dedicated variants (see `resolve_tyref`) so annotations match lambda/signal values.
            "Callable" => return Ty::Callable,
            "Signal" => return Ty::Signal(None),
            "Array" => return Ty::Array(Box::new(elem_arg(db, api, args, 0))),
            "Dictionary" => {
                return Ty::Dict(
                    Box::new(elem_arg(db, api, args, 0)),
                    Box::new(elem_arg(db, api, args, 1)),
                );
            }
            _ => {}
        }
        if let Some(b) = api.builtin_by_name(head) {
            return Ty::Builtin(b);
        }
        if let Some(c) = api.class_by_name(head) {
            return Ty::Object(c);
        }
        if let Some(e) = api.global_enum(head) {
            return Ty::Enum(EnumRef {
                qualified: SmolStr::new(head),
                bitfield: e.is_bitfield,
            });
        }
        // Unknown bare name → most likely another script's `class_name` → the seam.
        return resolve_external(db, &ExternalRef::ClassName(SmolStr::new(head)));
    }
    // Dotted: try `Class.Enum`; anything else (inner class, namespaced) is the seam.
    if names.len() == 2
        && let Some(c) = api.class_by_name(&names[0])
        && let Some(e) = api.class(c).enums.iter().find(|e| e.name == names[1])
    {
        return Ty::Enum(EnumRef {
            qualified: SmolStr::new(names.join(".")),
            bitfield: e.is_bitfield,
        });
    }
    resolve_external(db, &ExternalRef::ExtendsPath(SmolStr::new(names.join("."))))
}

/// Resolve the `i`-th type argument as a container element, collapsing a nested typed
/// container to `Variant` (Phase 2 does not track nested element types — Playbook §2). A
/// missing argument (bare `Array`/`Dictionary`) is `Variant`.
fn elem_arg(db: &dyn Db, api: &EngineApi, args: &[GdNode], i: usize) -> Ty {
    match args.get(i) {
        Some(node) => match resolve_type_ref(db, api, node) {
            Ty::Array(_) | Ty::Dict(..) => Ty::Variant,
            other => other,
        },
        None => Ty::Variant,
    }
}

/// Map a coarse engine-layer [`LayerTy`] (used by the hand-authored GDScript layer, which
/// predates the loaded model's real ids) to a [`Ty`].
#[must_use]
pub fn layer_to_ty(api: &EngineApi, lt: LayerTy) -> Ty {
    match lt {
        LayerTy::Float => builtin(api, "float"),
        LayerTy::Int => builtin(api, "int"),
        LayerTy::Bool => builtin(api, "bool"),
        LayerTy::Str => builtin(api, "String"),
        LayerTy::Array => Ty::array_of_variant(),
        LayerTy::Variant => Ty::Variant,
        LayerTy::Unknown => Ty::Unknown,
        LayerTy::Void => Ty::Void,
    }
}

fn builtin(api: &EngineApi, name: &str) -> Ty {
    api.builtin_by_name(name).map_or(Ty::Variant, Ty::Builtin)
}

// ---- base + class scope ------------------------------------------------------------------

/// Resolve a file's (or inner class's) base type from its `extends`. A bare engine-class name
/// resolves to `Object(id)`; a script-path / dotted / unknown base goes through the seam to
/// `Unknown`. With no `extends`, a script implicitly extends `RefCounted`.
#[must_use]
pub fn resolve_base(db: &dyn Db, api: &EngineApi, tree: &ItemTree, anchor: Option<&str>) -> Ty {
    match &tree.extends {
        None => api
            .class_by_name("RefCounted")
            .map_or(Ty::Unknown, Ty::Object),
        Some(ExtendsRef::Name(n)) => api.class_by_name(n).map_or_else(
            || resolve_external(db, &ExternalRef::ClassName(n.clone())),
            Ty::Object,
        ),
        // A string-path base (`extends "res://x.gd"` / `extends "sibling.gd"`): anchor a relative
        // path to the importing file's directory (Godot `get_base_dir().path_join()`), then resolve.
        Some(ExtendsRef::ScriptPath(p)) => match anchor_res_path(anchor, p) {
            Some(abs) => resolve_external(db, &ExternalRef::ExtendsPath(abs)),
            None => Ty::Unknown,
        },
        // A dotted base (`extends A.B`) is a namespaced name, not a path — the seam.
        Some(ExtendsRef::Path(p)) => resolve_external(db, &ExternalRef::ExtendsPath(p.clone())),
    }
}

/// What a class-level name resolves to within [`ClassScope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassItem {
    /// A declared member (index into [`ItemTree::members`]).
    Member(usize),
    /// A variant of an *anonymous* `enum { … }` (a class-level `int` constant).
    EnumVariant,
}

/// The class-member tier of the binder (Playbook §3.2 step 2): this file's own members + the
/// resolved base type. Anonymous-enum variants are flattened in as `int` constants.
#[derive(Debug, Clone)]
pub struct ClassScope<'a> {
    /// The lowered item tree this scope describes.
    pub tree: &'a ItemTree,
    /// The resolved base type (`Object(id)` for an engine base, else `Unknown`).
    pub base: Ty,
    /// The static type of `self` in this class's bodies. Defaults to [`base`](Self::base), but
    /// `analyze_file` overrides it with the script's *own* [`Ty::ScriptRef`] so that member access
    /// on an **aliased** `self` (`var me := self; me.own_method()`) walks the file's own members
    /// instead of only the engine base — otherwise a real own-method call would false-warn
    /// `UNSAFE_METHOD_ACCESS`. (Direct `self.member` already uses the own-member fast path.)
    pub self_ty: Ty,
    /// Resolved types of this class's own fields (`var`/`const`), seeded by a first inference
    /// pass over the field initializers so member references see the *inferred* type (e.g.
    /// `var n := 0` → `int`), not just the annotation. Empty until populated.
    pub member_types: FxHashMap<SmolStr, Ty>,
    members: FxHashMap<SmolStr, ClassItem>,
}

impl<'a> ClassScope<'a> {
    /// Build the scope for `tree` against the engine model.
    #[must_use]
    pub fn new(db: &dyn Db, api: &EngineApi, tree: &'a ItemTree, anchor: Option<&str>) -> Self {
        let mut members = FxHashMap::default();
        for (i, m) in tree.members.iter().enumerate() {
            match m {
                Member::Enum(e) if e.name.is_none() => {
                    // Anonymous enum: its variants become bare class-level `int` constants.
                    for v in &e.variants {
                        members.insert(v.clone(), ClassItem::EnumVariant);
                    }
                }
                _ => {
                    if let Some(name) = m.name() {
                        members
                            .entry(SmolStr::new(name))
                            .or_insert(ClassItem::Member(i));
                    }
                }
            }
        }
        let base = resolve_base(db, api, tree, anchor);
        Self {
            tree,
            self_ty: base.clone(),
            base,
            member_types: FxHashMap::default(),
            members,
        }
    }

    /// Resolve a name against this class's own members (not the base chain).
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<ClassItem> {
        self.members.get(name).copied()
    }

    /// The member behind a [`ClassItem::Member`].
    #[must_use]
    pub fn member(&self, item: ClassItem) -> Option<&'a Member> {
        match item {
            ClassItem::Member(i) => self.tree.members.get(i),
            ClassItem::EnumVariant => None,
        }
    }
}

// ---- global resolution -------------------------------------------------------------------

/// What a bare *global* name resolves to (Playbook §3.2 step 4). The caller ([`crate::infer`])
/// decides how to use it given the syntactic context (bare value vs. call vs. `.`-access).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalDef {
    /// A pseudo-constant value (`PI` → `float`).
    Const(Ty),
    /// An engine singleton instance (`Input` → `Object(Input)`).
    Singleton(ClassId),
    /// A GDScript builtin function (`preload`/`range`/`len`/…).
    Builtin,
    /// A `@GlobalScope` utility function (`sin`, `print`, …).
    Utility,
    /// A builtin Variant type name used as a value / constructor (`Vector2`, `int`).
    BuiltinType(BuiltinId),
    /// An engine class name used as a value / constructor / type (`Node`, `Resource`).
    ClassType(ClassId),
    /// A global enum namespace (`Error`, `Key`) — a set of `int` constants.
    GlobalEnum,
}

/// Resolve a bare global identifier. Order is deliberate: pseudo-constants and singletons take
/// precedence over the same-named type (bare `Input` is the singleton instance, not the class).
#[must_use]
pub fn resolve_global(api: &EngineApi, name: &str) -> Option<GlobalDef> {
    if let Some(gc) = api.global_const(name) {
        return Some(GlobalDef::Const(layer_to_ty(api, gc.ty)));
    }
    if let Some(cid) = api.singleton(name) {
        return Some(GlobalDef::Singleton(cid));
    }
    if api.gdscript_builtin(name).is_some() {
        return Some(GlobalDef::Builtin);
    }
    if api.utility(name).is_some() {
        return Some(GlobalDef::Utility);
    }
    if let Some(bid) = api.builtin_by_name(name) {
        return Some(GlobalDef::BuiltinType(bid));
    }
    if let Some(cid) = api.class_by_name(name) {
        return Some(GlobalDef::ClassType(cid));
    }
    if api.global_enum(name).is_some() {
        return Some(GlobalDef::GlobalEnum);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::item_tree;
    use gdscript_syntax::parse;

    fn api() -> &'static EngineApi {
        gdscript_api::bundled()
    }

    fn db() -> gdscript_db::RootDatabase {
        gdscript_db::RootDatabase::default()
    }

    /// Resolve the first `TypeRef` node found in `decl` source.
    fn ty_of_annotation(src: &str) -> Ty {
        let parse = parse(src);
        let root = parse.syntax_node();
        let type_ref = gdscript_syntax::ast::descendants(&root)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::TypeRef)
            .expect("a TypeRef node");
        resolve_type_ref(&db(), api(), &type_ref)
    }

    #[test]
    fn seam_is_unknown() {
        assert_eq!(
            resolve_external(&db(), &ExternalRef::ClassName(SmolStr::new("MyClass"))),
            Ty::Unknown
        );
    }

    #[test]
    fn builtin_and_class_annotations() {
        assert_eq!(
            ty_of_annotation("var x: int\n"),
            Ty::Builtin(api().builtin_by_name("int").unwrap())
        );
        assert_eq!(
            ty_of_annotation("var n: Node\n"),
            Ty::Object(api().class_by_name("Node").unwrap())
        );
        assert_eq!(ty_of_annotation("func f() -> void:\n\tpass\n"), Ty::Void);
    }

    #[test]
    fn typed_container_annotations() {
        let int = Ty::Builtin(api().builtin_by_name("int").unwrap());
        assert_eq!(
            ty_of_annotation("var a: Array[int]\n"),
            Ty::Array(Box::new(int.clone()))
        );
        assert_eq!(ty_of_annotation("var a: Array\n"), Ty::array_of_variant());
        assert_eq!(
            ty_of_annotation("var d: Dictionary[String, int]\n"),
            Ty::Dict(
                Box::new(Ty::Builtin(api().builtin_by_name("String").unwrap())),
                Box::new(int)
            )
        );
        // Nested typed containers collapse to Variant (Playbook §2).
        assert_eq!(
            ty_of_annotation("var a: Array[Array[int]]\n"),
            Ty::Array(Box::new(Ty::Variant))
        );
    }

    #[test]
    fn unknown_annotation_is_seam_not_error() {
        // A user `class_name` we can't see (no false diagnostic territory).
        assert_eq!(ty_of_annotation("var p: MyPlayer\n"), Ty::Unknown);
    }

    #[test]
    fn base_resolution() {
        let extends_node = item_tree(&parse("extends Node2D\n").syntax_node());
        assert_eq!(
            resolve_base(&db(), api(), &extends_node, None),
            Ty::Object(api().class_by_name("Node2D").unwrap())
        );
        // No extends → implicit RefCounted.
        let no_extends = item_tree(&parse("var x = 1\n").syntax_node());
        assert_eq!(
            resolve_base(&db(), api(), &no_extends, None),
            Ty::Object(api().class_by_name("RefCounted").unwrap())
        );
        // Script-path base with no project loaded → seam.
        let script_base = item_tree(&parse("extends \"res://b.gd\"\n").syntax_node());
        assert_eq!(resolve_base(&db(), api(), &script_base, None), Ty::Unknown);
    }

    #[test]
    fn anchor_res_path_absolute_passes_through() {
        assert_eq!(
            anchor_res_path(Some("res://a/b.gd"), "res://x.gd").as_deref(),
            Some("res://x.gd")
        );
        assert_eq!(
            anchor_res_path(None, "user://x.gd").as_deref(),
            Some("user://x.gd")
        );
    }

    #[test]
    fn anchor_res_path_relative_anchors_to_importing_dir() {
        let from = Some("res://entities/player.gd");
        // sibling
        assert_eq!(
            anchor_res_path(from, "enemy.gd").as_deref(),
            Some("res://entities/enemy.gd")
        );
        // parent traversal (`..`) collapses
        assert_eq!(
            anchor_res_path(from, "../core/hooks.gd").as_deref(),
            Some("res://core/hooks.gd")
        );
        // explicit current-dir (`./`)
        assert_eq!(
            anchor_res_path(from, "./util.gd").as_deref(),
            Some("res://entities/util.gd")
        );
        // an importer at the project root
        assert_eq!(
            anchor_res_path(Some("res://main.gd"), "util.gd").as_deref(),
            Some("res://util.gd")
        );
    }

    #[test]
    fn anchor_res_path_relative_without_anchor_is_seam() {
        assert_eq!(anchor_res_path(None, "sibling.gd"), None);
    }

    #[test]
    fn class_scope_members_and_anon_enum() {
        let tree = item_tree(
            &parse(
                "var hp := 10\nfunc attack():\n\tpass\nenum { FIRE, ICE }\nenum Named { A, B }\n",
            )
            .syntax_node(),
        );
        let scope = ClassScope::new(&db(), api(), &tree, None);
        assert!(matches!(scope.lookup("hp"), Some(ClassItem::Member(_))));
        assert!(matches!(scope.lookup("attack"), Some(ClassItem::Member(_))));
        // Anonymous-enum variants flatten into the class scope as int consts.
        assert_eq!(scope.lookup("FIRE"), Some(ClassItem::EnumVariant));
        assert_eq!(scope.lookup("ICE"), Some(ClassItem::EnumVariant));
        // A named enum binds its *name*, not its variants.
        assert!(matches!(scope.lookup("Named"), Some(ClassItem::Member(_))));
        assert_eq!(scope.lookup("A"), None);
    }

    #[test]
    fn globals() {
        assert!(matches!(
            resolve_global(api(), "PI"),
            Some(GlobalDef::Const(_))
        ));
        assert!(matches!(
            resolve_global(api(), "Input"),
            Some(GlobalDef::Singleton(_))
        ));
        assert!(matches!(
            resolve_global(api(), "preload"),
            Some(GlobalDef::Builtin)
        ));
        assert!(matches!(
            resolve_global(api(), "Vector2"),
            Some(GlobalDef::BuiltinType(_))
        ));
        assert!(matches!(
            resolve_global(api(), "Node"),
            Some(GlobalDef::ClassType(_))
        ));
        assert!(resolve_global(api(), "definitely_not_a_global").is_none());
    }
}
