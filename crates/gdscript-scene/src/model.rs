//! The parsed scene/resource model produced by [`crate::parse_scene`] (Phase-4 M0).
//!
//! Pure data + read-only lookups — **no `FileId`, no db, no engine model**. M1 wraps the parser in
//! a salsa query and maps the recorded `type=`/`script=`/`instance=` data onto a `Ty`; M0 only
//! records the structure + byte spans (for go-to-definition into the `.tscn`).
//!
//! The shape follows `PHASE-4-M0-PLAYBOOK.md` §3, using the workspace conventions
//! ([`SmolStr`]/[`FxHashMap`]) — not the playbook prose's `EcoString` (which the crate does not use).

use gdscript_base::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

/// Index of a node into [`SceneModel::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeIdx(pub u32);

/// An `ext_resource`/`sub_resource` id — the opaque, quoted-string key as written
/// (`"1"`, `"1_app"`, `"StyleBoxFlat_x"`). A 3.x bare-int id is normalized to its string form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExtId(pub SmolStr);

/// Whether the parsed file is a `.tscn` scene (`gd_scene`) or a `.tres` resource (`gd_resource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneKind {
    /// A `.tscn` scene — has a `[node]` tree.
    Scene,
    /// A `.tres` resource — has a `[resource]` body, no node tree.
    Resource,
}

/// One parsed `.tscn`/`.tres`. Produced by [`crate::parse_scene`]; **never** an `Err` — every
/// malformed/binary/unknown form degrades to an empty-or-partial model plus a [`SceneProblem`].
/// `PartialEq`/`Eq` so it can be a backdated salsa query result (`Arc<SceneModel>`) in M1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneModel {
    /// Scene vs resource (from the header tag).
    pub kind: SceneKind,
    /// The `format=` version: `>=3` is the Godot-4.x family, `2` is 3.x, `None` if absent.
    pub format: Option<u8>,
    /// The scene/resource `uid="uid://…"`, if present.
    pub uid: Option<SmolStr>,
    /// The header `script_class="…"` shortcut — the root/resource's `class_name` without resolving
    /// the script file.
    pub script_class: Option<SmolStr>,
    /// A `.tres`'s own resource class (`gd_resource type="…"`).
    pub resource_type: Option<SmolStr>,

    /// `id → ext_resource` (external scripts, packed scenes, textures, …).
    pub ext_resources: FxHashMap<ExtId, ExtResource>,
    /// `id → sub_resource` type (the value body is skipped — we keep only the declared type).
    pub sub_resources: FxHashMap<ExtId, SubResource>,

    /// Every node, in file order (which is tree pre-order for siblings). `index = NodeIdx.0`.
    pub nodes: Vec<SceneNode>,
    /// The single parent-less node, if the scene has one.
    pub root: Option<NodeIdx>,

    /// Full name-path from the root (`"Panel/VBox/StartButton"`, root name excluded) → node.
    pub by_path: FxHashMap<SmolStr, NodeIdx>,
    /// `unique_name_in_owner` nodes: bare name → node (the `%Name` lookup; scene-wide in the slice).
    pub unique_nodes: FxHashMap<SmolStr, NodeIdx>,

    /// Non-fatal problems found while parsing (the parser never errors).
    pub problems: Vec<SceneProblem>,

    /// `(parent, child-name) → child` — the segment-by-segment path walk index. Built in pass 2.
    child_index: FxHashMap<(NodeIdx, SmolStr), NodeIdx>,
    /// `parent → ordered children` — for `$`/`get_node` child completion.
    children: FxHashMap<NodeIdx, Vec<NodeIdx>>,
}

/// One `[node …]` section: its header attributes + the two body properties we read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneNode {
    /// The node name (unescaped; may contain spaces).
    pub name: SmolStr,
    /// `type="X"` — the declared class (native **or** a custom `class_name`). `None` ⇒ instanced.
    pub decl_type: Option<SmolStr>,
    /// The raw `parent="…"` path (`"."` = child of root; relative, root-excluded). `None` ⇒ root.
    pub parent_path: Option<SmolStr>,
    /// The resolved parent (pass 2). `None` ⇒ root, or an unresolved/dangling parent.
    pub parent_idx: Option<NodeIdx>,
    /// Body `script = ExtResource("id")`.
    pub script: Option<ExtId>,
    /// Header `instance=ExtResource("id")` (an instanced sub-scene; its type comes from that scene).
    pub instance: Option<ExtId>,
    /// `instance=` on a parent-less node ⇒ an *inherited* scene (`set_base_scene`), not a child.
    pub instance_is_inherited_root: bool,
    /// `instance_placeholder="res://…"` (the lazy-instance variant).
    pub instance_placeholder: bool,
    /// Body `unique_name_in_owner = true` (the `%Name` marker — distinct from the header `unique_id`).
    pub unique_name_in_owner: bool,
    /// Byte span of the whole `[node …]` header line (coarse go-to-definition).
    pub header_span: TextRange,
    /// Byte span of the `name="…"` value (finer go-to-definition / highlight).
    pub name_span: TextRange,
}

/// An `[ext_resource …]` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtResource {
    /// `type="Script" | "PackedScene" | "Texture2D" | …`.
    pub res_type: SmolStr,
    /// `path="res://…"`, if present (prefer this over `uid` in the slice).
    pub path: Option<SmolStr>,
    /// `uid="uid://…"`, if present (resolved via the project UID map in M1).
    pub uid: Option<SmolStr>,
    /// Byte span of the `[ext_resource …]` header line.
    pub span: TextRange,
}

/// A `[sub_resource …]` declaration (type only — the value body is skipped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubResource {
    /// `type="…"`.
    pub res_type: SmolStr,
    /// Byte span of the header line.
    pub span: TextRange,
}

/// A non-fatal problem found during parsing. The parser records these and keeps going — the floor
/// is always parity with the engine's `Node`-everywhere baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneProblem {
    /// A binary `.scn`/`.res` (RSRC/RSCC magic) — detected and degraded to an empty model.
    BinaryResource,
    /// A section tag not in the 8 the engine recognizes — the section was skipped.
    UnknownTag {
        /// The header line span.
        at: TextRange,
    },
    /// A bracketed header that could not be lexed — the section was skipped.
    MalformedHeader {
        /// The (best-effort) header span.
        at: TextRange,
    },
    /// An `ext_resource` missing a required `type`/`path`/`id`.
    MissingExtField {
        /// The header line span.
        at: TextRange,
    },
    /// A `script=`/`instance=` referencing an id with no matching `ext_resource`.
    UnknownExtResource {
        /// The dangling id.
        id: ExtId,
        /// The referencing node's header span.
        at: TextRange,
    },
    /// More than one parent-less node (the first is kept as root).
    MultipleRoots {
        /// All parent-less nodes.
        roots: Vec<NodeIdx>,
    },
    /// A scene with `[node]`s but no parent-less node.
    NoRoot,
    /// A node whose `parent="…"` path resolves to no known node.
    DanglingParent {
        /// The orphaned node.
        node: NodeIdx,
        /// The unresolved parent path.
        parent_path: SmolStr,
    },
}

impl SceneModel {
    /// An empty model of `kind` (the degrade target).
    #[must_use]
    pub(crate) fn empty(kind: SceneKind) -> Self {
        Self {
            kind,
            format: None,
            uid: None,
            script_class: None,
            resource_type: None,
            ext_resources: FxHashMap::default(),
            sub_resources: FxHashMap::default(),
            nodes: Vec::new(),
            root: None,
            by_path: FxHashMap::default(),
            unique_nodes: FxHashMap::default(),
            problems: Vec::new(),
            child_index: FxHashMap::default(),
            children: FxHashMap::default(),
        }
    }

    /// Set the pass-2-built navigation indices (called by the parser).
    pub(crate) fn set_indices(
        &mut self,
        child_index: FxHashMap<(NodeIdx, SmolStr), NodeIdx>,
        children: FxHashMap<NodeIdx, Vec<NodeIdx>>,
    ) {
        self.child_index = child_index;
        self.children = children;
    }

    /// The node at `idx`, if it exists.
    #[must_use]
    pub fn node(&self, idx: NodeIdx) -> Option<&SceneNode> {
        self.nodes.get(idx.0 as usize)
    }

    /// Walk a name-path from the scene root. `""`/`"."` ⇒ the root. `None` ⇒ no such node (M1 reads
    /// that as "degrade to `Node`", never an error). A leading `/` (absolute) or a `..` segment is
    /// out of the slice and yields `None`.
    #[must_use]
    pub fn resolve_path(&self, path: &str) -> Option<NodeIdx> {
        self.resolve_path_from(self.root?, path)
    }

    /// Walk a name-path from an arbitrary `base` node (the node a script attaches to — `$X` is
    /// relative to *that* node, which is usually but not always the root).
    #[must_use]
    pub fn resolve_path_from(&self, base: NodeIdx, path: &str) -> Option<NodeIdx> {
        let p = path.trim();
        if p.is_empty() || p == "." {
            return Some(base);
        }
        if p.starts_with('/') {
            return None; // absolute /root/... — out of the slice
        }
        let mut cur = base;
        for seg in p.split('/') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                return None; // parent escape — needs the runtime tree
            }
            cur = *self.child_index.get(&(cur, SmolStr::new(seg)))?;
        }
        Some(cur)
    }

    /// The `%Name` lookup — a node marked `unique_name_in_owner` (the leading `%` is stripped by the
    /// caller). Scene-wide in the slice.
    #[must_use]
    pub fn resolve_unique(&self, name: &str) -> Option<NodeIdx> {
        self.unique_nodes.get(name).copied()
    }

    /// The node whose body `script = ExtResource(id)` resolves (via `ext_resources[id].path`) to
    /// `script_path` — the per-scene half of the script↔scene association.
    #[must_use]
    pub fn node_with_script(&self, script_path: &str) -> Option<NodeIdx> {
        self.nodes.iter().enumerate().find_map(|(i, n)| {
            let ext = self.ext_resources.get(n.script.as_ref()?)?;
            (ext.path.as_deref() == Some(script_path))
                .then(|| NodeIdx(u32::try_from(i).unwrap_or(u32::MAX)))
        })
    }

    /// The child nodes of `idx` (`None` ⇒ the root's children), in file order.
    pub fn children_of(&self, idx: Option<NodeIdx>) -> impl Iterator<Item = (NodeIdx, &SceneNode)> {
        idx.or(self.root)
            .and_then(|t| self.children.get(&t))
            .into_iter()
            .flatten()
            .filter_map(move |&c| self.node(c).map(|n| (c, n)))
    }
}
