# PHASE 4 — Scene Awareness (Tier 3 slice) — ★ THE KILLER FEATURE ★

> **Status:** plan. **Tier:** 3 (slice). **Delivers:** node-path typing the Godot editor LSP cannot produce — `$Panel/VBox/StartButton` infers `Button`, with **zero annotations**.
> **Canonical parents this doc obeys:** [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§1 crate stack — the new `gdscript-scene` crate; §2 the `AnalysisHost`/`Analysis` API; §5 data model; §7 portability), [`ROADMAP.md`](ROADMAP.md) (Phase 4 deliverable + exit criteria, Tier 3 slice, "pull a slice forward after Phase 2").
> **Primary evidence:** [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) **§5 (Scene awareness — the killer feature)**, §2.4 (node-path typing), §3.4/§7 (the early-slice recommendation); [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) §1.9 (`$ % &"" ^"" get_node`), §1.10 (`@onready`), §3.4 (the `.tscn` extraction model), §3.7 (authoritative vs derived files).

This phase turns "we understand a single GDScript file and its project graph" (Phases 2–3) into **"we understand the *scene* a script is attached to."** It is the project's **signature capability** — the one feature where we are not catching up to the Godot editor LSP but **decisively beating it**. Godot's own tooling statically types every `$Path`, `%Unique`, and `get_node("...")` as bare `Node` because, per the engine's own docs and GDQuest's analysis, "the language server can't automatically know what type of node it is." We *can* know — the type is sitting right there in the owning `.tscn`. We read it.

---

## Goal & scope

### The killer feature, in one line

> A `.gd` script attached to a scene gets its `$NodePath` / `%Unique` / `get_node("...")` expressions **typed from the scene's node tree** — `$Panel/VBox/StartButton` resolves to `Button`, completion offers the *actual child node names*, an invalid path warns, and the attached script refines the type. No `as` cast, no annotation, no running editor.

### What ships (the deliverable)

1. **`gdscript-scene`** — a new core crate: a robust `.tscn`/`.tres` **text parser** producing a `SceneModel` (a node tree carrying each node's name, declared `type=`, parent path, attached-script ext_resource, `unique_name_in_owner` flag, and instanced-sub-scene ext_resource), plus the `ext_resource`/`sub_resource` tables. Read-only; never rewrites.
2. **Script↔scene association** — resolve which scene(s) own a given `.gd` (the node whose `script = ExtResource(...)` points at it), living in the Phase-3 project model; the slice can take a single explicitly-associated scene.
3. **Node-path typing in `gdscript-hir`** — `$Path`, `%Unique`, `get_node("..."/NodePath literal)`, and `@onready var x = $Path` resolve to the **concrete node class** by walking the owning scene's `SceneModel`, refined by the node's attached script. Falls back to `Node` when unknown — **never regresses**.
4. **Scene-aware IDE features** — invalid-path diagnostics, node-name completion inside `$`/`get_node`, hover showing the resolved type, and go-to-definition from `$Path` into the `.tscn` node.

### The 90% slice vs the hard tail

The research is explicit ([`research/09`](research/09-type-system-and-inference.md) §5.3, §7): **ship the 90% solution first.** The split:

| | **The 90% slice (ship first)** | **The hard tail (the multi-year polish)** |
|---|---|---|
| Node type source | Direct `type="..."` on the node in the **single owning scene** | Recursive **instanced sub-scene** root types; **inherited scenes** |
| Scene resolution | One owning scene (the common case) | **1-script-many-scenes** ambiguity; multi-scene union/primary |
| Path forms | `$A/B/C`, `get_node("A/B/C")` literal | `%Unique` across owner boundaries inside instanced sub-scenes; `..`/absolute `/root/...`; dynamic/computed paths |
| Resource refs | `path="res://..."` direct | `uid://` indirection (resolve via the project UID map) |
| Refinement | Attached script's `class_name`/native base | Cross-scene `@onready` timing nuance; `_ready` flow |

The slice needs only **the owning scene + the Phase-2 type layer + the API model** — it does **not** need the full Phase-3 project graph. That is what makes it pullable forward (see Workstream 5). The tail needs Phase 3's project model (UID map, the script→scene index, instanced-scene recursion across files).

### What "pull a slice forward after Phase 2" means

[`ROADMAP.md`](ROADMAP.md) (Tier→Phase table; "early scene slice possible") and [`research/09`](research/09-type-system-and-inference.md) §7 ("recommended highest-value-earliest path: Tier 0 → Tier 1 → **early slice of Tier 3 scene typing** → Tier 2 → rest of Tier 3"). Concretely: a **thin `gdscript-scene` + a single explicitly-associated scene + direct `type=` typing of `$Path`** can land **right after Phase 2 ships the MVP**, as a wow-demo, *before* the heavy Phase-3 project-graph incrementality. It reuses the Phase-2 seam exactly (`GetNode` already returns `Node`; the slice sharpens that one expression rule) and needs no salsa. Workstream 5 specifies the minimal cut. The full Phase-4 deliverable then folds in the hard tail once Phase 3's project model exists.

### Explicit non-goals (deferred)

| Deferred | Where | Why |
|---|---|---|
| Binary `.scn` scenes | out of scope (v1+) | Binary; needs a full `ResourceFormatBinary` reader. Text `.tscn` is the editor default for VCS; cover it. We *detect* `.scn` and degrade to `Node` (never error). |
| Evaluating resource *values* (`sub_resource` property graphs, `.tres` data) | out of scope | We need node **types/scripts/structure**, not deserialized resource values. Parse the headers, skip the value bodies. |
| Full `@onready`/`_ready` control-flow validation (using `$` before the node exists) | Phase 6 | Timing diagnostics (`GET_NODE_DEFAULT_WITHOUT_ONREADY` is Phase-2/6 territory) ride the flow narrowing upgrade. Phase 4 types the path; it does not model node-lifetime flow. |
| Scene-tree *editing* / rename-across-scenes (rename a node, fix `$Path`s) | Phase 6 | Needs the write path + cross-file rename engine. Phase 4 is read/typing only. |

---

## Why this matters

This is the section that justifies the project's existence. **Every other feature in the roadmap brings us to parity with a good language server. This one puts us ahead of the engine's own tooling.**

### The concrete user pain (today)

In Godot today, this is the universal experience writing UI or gameplay code:

```gdscript
# main_menu.gd — attached to MainMenu.tscn
@onready var start_button = $Panel/VBox/StartButton   # <- statically typed as Node

func _ready() -> void:
    start_button.text = "Play"        # no completion for `.text`; no check it exists
    start_button.pressed.connect(_on_start)   # `.pressed` unknown; unsafe access
```

`start_button` is `Node`. `Node` has no `text` and no `pressed` signal — those live on `Button`. So:
- **Completion is useless** — typing `start_button.` offers `Node`'s members, not `Button`'s. The one member you want (`text`, `pressed`, `disabled`, `toggle_mode`) is never offered.
- **Nothing is checked** — `start_button.txet = "Play"` (typo) is silently accepted; it explodes at runtime.
- **The fix is manual ceremony** — the documented, idiomatic workaround is to **hand-write the cast** on every node access:
  ```gdscript
  @onready var start_button := $Panel/VBox/StartButton as Button   # the tax users pay
  ```
  This is the `as`-cast tax. GDQuest documents it as *the* node-typing workaround precisely because the tooling forces it. It is noise, it goes stale when scenes change, and beginners don't know to do it — so most real projects are a sea of untyped `Node` access.

### What we do instead

**The type IS knowable.** The owning scene file says so, in plain text:

```gdscene
# MainMenu.tscn
[node name="Panel"  type="Panel"        parent="."]
[node name="VBox"   type="VBoxContainer" parent="Panel"]
[node name="StartButton" type="Button"  parent="Panel/VBox"]   # <- the type, right here
```

We parse `MainMenu.tscn`, find that `main_menu.gd` is attached (`script = ExtResource` → `main_menu.gd`), build the node tree, walk `Panel/VBox/StartButton`, and read `type="Button"`. So:

```gdscript
@onready var start_button = $Panel/VBox/StartButton   # WE infer: Button — zero annotations

func _ready() -> void:
    start_button.text = "Play"             # `.text`: String — completed, checked
    start_button.pressed.connect(_on_start)  # `.pressed`: Signal — completed, checked
    start_button.txet = "Play"             # ← we flag the typo (UNSAFE_PROPERTY_ACCESS)
```

And we go further than just typing:
- **`$`/`get_node` completion offers the actual child node names** — type `$Panel/` and we list `VBox` (and its siblings) **from the scene**, not a blank. No other GDScript tool does this.
- **`$DoesNotExist` warns** — a node path that isn't in the scene is a diagnostic, caught before runtime.
- **Go-to-definition on `$Path`** jumps into the `.tscn`, to the `[node ...]` line.

### Why this beats the engine LSP (and why it can)

The engine LSP **architecturally cannot** do this well, and the gap is documented:

- The engine's own static typing docs and [GDQuest's type-inference glossary](https://www.gdquest.com/library/glossary/type_inference/) state plainly that `$Path`/`get_node` "**return `Node`**… the language server can't automatically know what type of node it is" — hence the recommended `$Char as CharacterBody2D` workaround. ([`research/09`](research/09-type-system-and-inference.md) §2.4.)
- The community refactor proposal [godot-proposals #11056](https://github.com/godotengine/godot-proposals/issues/11056) ("refactor GDScript Language Server") and the broader LSP-limitations discussion exist *because* the editor-hosted LSP is structurally limited here ([`research/04`](research/04-gdscript-semantics-and-features.md) §5).
- The engine LSP **runs inside the editor** (TCP 6005; "a Godot instance must be running on your current project" — [`research/04`](research/04-gdscript-semantics-and-features.md) §5.3). Even where it *could* peek at scene data, it ties analysis to a live editor. **We run standalone, headless, in CI** — and still type the node, because we read the `.tscn` ourselves.

The research's verdict ([`research/09`](research/09-type-system-and-inference.md) §5.3): scene typing is *"the feature most likely to make users say 'it's smarter than the built-in editor.'"* And §2.4: it is *"the single highest-leverage Godot-specific feature."* This phase is where the product's headline — **a Roslyn for Godot, not a smarter linter** — actually lands.

---

## Prerequisites

| For the **90% slice** (pullable forward after Phase 2) | For the **full Phase 4** (the hard tail) |
|---|---|
| **Phase 2 type layer** ([`PHASE-2`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md)): `Ty`, the `EngineApi` inheritance table (to map `type="Button"` → `Ty::Object(ClassId)` and to refine by an attached script's native base), and the `Expr::GetNode { path }` HIR node that **already returns `Node`** — the slice sharpens this one rule. | **Phase 3 project model** ([`PHASE-3`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md)): the workspace VFS scan, the `res://` ↔ FileId mapper, the **UID map** (`uid://` → path, rebuilt from authoritative files — [`research/04`](research/04-gdscript-semantics-and-features.md) §3.7), the `class_name` registry (to refine a node to its attached script's `class_name` type), and **salsa** (so `scene_node_tree(scene)` is a cached query invalidated only when that `.tscn` changes). |
| **`gdscript-base`**: `FileId`, `TextRange`, `LineIndex`. The scene parser emits byte spans for go-to-def into `.tscn`. | The **script→scene index** (a project-wide reverse map: `.gd` FileId → the scenes that attach it) — the home of the association logic (Workstream 2). |
| A way to hand the owning scene's **text** to the analyzer via `apply_change` (the slice can take an explicit "this `.gd` is owned by this `.tscn`" association, or a single same-directory `.tscn`, without the full scan). | Instanced-sub-scene recursion needs the project graph to follow `instance=ExtResource("res://enemy.tscn")` → parse `enemy.tscn` → its root type. |

**Portability gate (non-negotiable — [`01`](01-ARCHITECTURE.md) §7):** `gdscript-scene` is a **core crate** and **must** compile to `wasm32`. **No `std::fs`** — `.tscn` text is injected through the VFS exactly like `.gd` text; **no `Instant::now`**, **no threads** in the parser. CI runs `cargo check -p gdscript-scene --target wasm32-unknown-unknown`.

**Sanity gate before starting:** Phase-2 exit criteria green (member completion after `button.` works via the inheritance table — the same machinery that types a resolved node).

---

## Workstream 1 — `gdscript-scene`: the `.tscn`/`.tres` parser

The new crate. A small, robust, **read-only** text parser. It does **not** model resource values, does not rewrite, and is **lossless enough for our needs**: we read node structure, types, scripts, and resource refs; we ignore property bodies we don't type from.

### 1.1 The format spec (what we parse)

`.tscn` is a Godot-flavored **INI-like** text format ([TSCN docs](https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html); [`research/09`](research/09-type-system-and-inference.md) §5.1, [`research/04`](research/04-gdscript-semantics-and-features.md) §3.4). Structure: a sequence of **sections**, each a bracketed **header** line `[section_type key=value ...]` optionally followed by `key = value` **property** lines until the next header or EOF.

```gdscene
[gd_scene load_steps=4 format=3 uid="uid://cecaux1sm7mo0"]

[ext_resource type="Script"      path="res://main_menu.gd"  id="1_abc12"]
[ext_resource type="PackedScene" path="res://enemy.tscn"    id="2_xyz99"]
[ext_resource type="Texture2D"   uid="uid://b8k..."         path="res://icon.png" id="3_t"]

[sub_resource type="StyleBoxFlat" id="StyleBoxFlat_1"]
bg_color = Color(0.1, 0.1, 0.1, 1)

[node name="MainMenu" type="Control"]
script = ExtResource("1_abc12")

[node name="Panel"  type="Panel"         parent="."]
[node name="VBox"   type="VBoxContainer" parent="Panel"]
[node name="StartButton" type="Button"   parent="Panel/VBox" unique_name_in_owner=true]

[node name="Enemy"  parent="."  instance=ExtResource("2_xyz99")]
```

**The section grammar (what the tokenizer recognizes):**

| Section header | Carries | We extract |
|---|---|---|
| `[gd_scene load_steps=N format=3 uid="uid://..."]` | scene header | `format` (3 = Godot 4.x — the version discriminator); scene `uid` |
| `[gd_resource type="..." ... format=3 uid=...]` | `.tres` header | marks a resource file (Workstream 1.5) |
| `[ext_resource type="..." path="res://..." uid="uid://..." id="..."]` | external resource ref | `id → { type, path?, uid? }`; **`type="Script"`** is the script-attach key; **`type="PackedScene"`** is the sub-scene key |
| `[sub_resource type="..." id="..."]` + body | inline resource | `id → type` only; **skip the property body** (we don't type from sub-resource values) |
| `[node name="..." type="..." parent="..." index=N groups=[...] instance=ExtResource("id") instance_placeholder="..."]` | a node | `name`, `type?`, `parent?`, `instance?` (sub-scene), and from the body: `script = ExtResource(id)`, `unique_name_in_owner = true` |
| `[connection signal="..." from="..." to="..." method="..."]` | a signal wire | **ignored** for typing (kept parseable; future: signal go-to) |
| `[editable path="..."]` | sub-scene override marker | parsed, ignored for the slice |

**Node header attributes and the two body keys that matter** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.4):
- **`name`** — required; the path segment.
- **`type="X"`** — the node's Godot class. **Absent** when the node is an instanced sub-scene.
- **`parent="..."`** — `"."` = child of root; `"Panel/VBox"` = relative path (the **root name is excluded** from these paths); **absent** ⇒ this is the **root** node (exactly one node has no `parent`).
- **`instance=ExtResource("id")`** — this node is an instanced sub-scene; its type/children come from the referenced `PackedScene` (the hard-tail recursion).
- Body `script = ExtResource("id")` — attached script (upgrades the node's type from its engine class to the script class).
- Body `unique_name_in_owner = true` — the node is a `%Unique` target within its owner.

### 1.2 The parsed model (`SceneModel` / `NodeTree`)

```rust
// crates/gdscript-scene/src/model.rs  (sketch — illustrative, not final)

/// One parsed .tscn (or .tres). Built once per scene file; in Phase 3 it is a salsa-cached
/// query `scene_model(db, FileId) -> Arc<SceneModel>`, invalidated only when that file changes.
pub struct SceneModel {
    pub kind: SceneKind,                       // Scene (.tscn) | Resource (.tres)
    pub format: u8,                            // 3 = Godot 4.x; other/absent => degrade gracefully
    pub uid: Option<EcoString>,                // the scene's own uid://...
    pub ext_resources: FxHashMap<ExtId, ExtResource>,  // "1_abc12" -> {type, path?, uid?}
    pub sub_resources: FxHashMap<EcoString, EcoString>,// id -> type (no value bodies)
    pub nodes: Vec<SceneNode>,                 // flat arena; index = NodeIdx
    pub root: Option<NodeIdx>,                 // the parent-less node
    /// name-path ("Panel/VBox/StartButton") -> NodeIdx, precomputed for O(1) path walks.
    pub by_path: FxHashMap<EcoString, NodeIdx>,
    /// unique_name_in_owner nodes: "%StartButton" target -> NodeIdx (per owner; slice = scene-wide).
    pub unique_nodes: FxHashMap<EcoString, NodeIdx>,
    /// non-fatal parse problems (for diagnostics on the .tscn itself + robustness telemetry).
    pub problems: Vec<SceneProblem>,
}

#[derive(Copy, Clone, PartialEq, Eq)] pub struct NodeIdx(u32);
#[derive(Clone, PartialEq, Eq, Hash)] pub struct ExtId(EcoString);   // the resource id string

pub enum SceneKind { Scene, Resource }

pub struct SceneNode {
    pub name: EcoString,                       // "StartButton"
    pub decl_type: Option<EcoString>,          // type="Button"  (None if instanced)
    pub parent_idx: Option<NodeIdx>,           // resolved from parent="..." (None = root)
    pub parent_path: Option<EcoString>,        // raw "Panel/VBox" | "." | None
    pub script: Option<ExtId>,                 // script = ExtResource("1_abc12")
    pub instance: Option<ExtId>,               // instance=ExtResource("2_xyz99") -> sub-scene
    pub unique_name_in_owner: bool,
    pub header_span: TextRange,                 // byte span of the `[node ...]` line (go-to-def target)
}

pub struct ExtResource {
    pub res_type: EcoString,                    // "Script" | "PackedScene" | "Texture2D" | ...
    pub path: Option<EcoString>,                // "res://main_menu.gd"  (may be absent if only uid given)
    pub uid:  Option<EcoString>,                // "uid://..."  (resolve via project UID map — hard tail)
    pub span: TextRange,
}

pub enum SceneProblem {
    MalformedHeader(TextRange),
    UnknownExtResource { id: ExtId, at: TextRange },  // script=/instance= pointing at a missing id
    MultipleRoots(Vec<NodeIdx>),
    DanglingParent { node: NodeIdx, parent_path: EcoString },
}
```

The public surface the checker uses:

```rust
impl SceneModel {
    /// Walk a node-name path ("Panel/VBox/StartButton") from the scene root. None = no such node.
    pub fn resolve_path(&self, path: &str) -> Option<NodeIdx>;      // handles "." , "A/B", trailing/empty
    /// Find a unique-name node ("%StartButton" or "StartButton"). Slice: scene-wide; tail: per-owner.
    pub fn resolve_unique(&self, name: &str) -> Option<NodeIdx>;
    /// The scene node that attaches `script_path` (Workstream 2's per-scene half).
    pub fn node_with_script(&self, script_path: &str) -> Option<NodeIdx>;
    pub fn node(&self, idx: NodeIdx) -> &SceneNode;
    /// Child node names of a node (for $/get_node completion).
    pub fn children_of(&self, idx: Option<NodeIdx>) -> impl Iterator<Item = &SceneNode>;
}
```

### 1.3 The parse approach

A **two-pass, line-oriented, error-tolerant** scan — not a grammar-heavy parser. The format is regular enough that a small hand-written tokenizer beats pulling in a generic INI crate (which mishandles Godot's `Type(...)` value syntax, `[]` group arrays, and bracketed headers).

1. **Pass 1 — sectionize.** Scan lines; a line whose first non-space char is `[` and that closes with `]` opens a section (record its byte span). Parse the header's `key=value` attributes with a tiny value lexer that understands: quoted strings (`"..."`, handling escapes), bare idents/ints/bools, `ExtResource("id")` / `SubResource("id")` constructor calls, `[...]` arrays (skipped), and `Type(args)` constructors (skipped — we never need the value). Lines between headers that look like `key = value` attach to the current section; we only *read* `script`, `unique_name_in_owner` (and on `ext_resource` headers, `type`/`path`/`uid`/`id`); everything else (including multi-line resource bodies) is **skipped without parsing its value** — we just advance to the next header.
2. **Pass 2 — build the tree.** From the collected `[node ...]` sections: the parent-less node is `root`; resolve each `parent=` path against the accumulating tree to set `parent_idx`; compute the full name-path for `by_path`; collect `unique_name_in_owner` nodes into `unique_nodes`. Resolve each node's `script`/`instance` `ExtResource("id")` against the `ext_resources` table.

**Robustness is a feature, not a nicety.** Real `.tscn` files in the wild contain comments, custom resource types, plugin-written sections, trailing whitespace, CRLF, and version drift. The parser **never panics and never hard-fails**: anything it can't interpret becomes a `SceneProblem` (logged, optionally surfaced as a `.tscn` diagnostic) and the node typing for affected paths **degrades to `Node`**. A scene we can't parse at all yields an empty `SceneModel` → every `$Path` falls back to `Node` (Phase-2 behavior). *We are strictly additive over the engine's `Node`-everywhere baseline; the worst case is parity.*

### 1.4 Edge cases (Workstream 1)

| Edge case | Handling |
|---|---|
| **`uid://` ext_resource (uid-only, no `path`)** | Slice: prefer `path=`; if only `uid=`, defer (→ `Node`). Tail: resolve via the Phase-3 project **UID map** (rebuilt from authoritative `uid="..."` strings — [`research/04`](research/04-gdscript-semantics-and-features.md) §3.7), never the stale `.godot/uid_cache.bin`. |
| **Missing referenced file** (`script`/`instance` points at a path not in the VFS) | `SceneProblem::UnknownExtResource`; that node's script-refinement / sub-scene-type is skipped → falls back to `decl_type` or `Node`. No crash, no false type. |
| **Binary `.scn`** | Detect (non-text / wrong magic); produce an empty `SceneModel` with a `problems` note. Out of scope to parse; degrade to `Node`. |
| **`format != 3`** (Godot 3.x `format=2`, or future `format=4`) | Parse best-effort (the node grammar is stable); record `format`. We target `format=3` (4.x) per [`research/04`](research/04-gdscript-semantics-and-features.md) §3.4; older/newer → best-effort, degrade where node shape differs. |
| **Quoted/spaced node names** (`$"Panel/My Button"`, `name="My Button"`) | Names are stored verbatim; path matching is exact-string per segment (spaces allowed). The `$`-path lexer (Workstream 3) honors the quoted form. |
| **Multiple roots / no root / cyclic parent** | `SceneProblem`; pick the first parent-less node as root (or none); never loop. |
| **CRLF / tabs / trailing comments (`;`)** | Line scanner normalizes line endings; strips trailing `;` comments outside quotes. |

### 1.5 `.tres` for resources/scripts

`.tres` shares the grammar (`[gd_resource ...]`, `[ext_resource ...]`, `[sub_resource ...]`) — [`research/04`](research/04-gdscript-semantics-and-features.md) §3.5. The same parser handles it (`SceneKind::Resource`). For Phase 4 we parse `.tres` only to (a) follow a `.tres` that is itself a script resource or references one, and (b) keep the `ext_resource` table consistent. We do **not** type from resource property values. (A `.tres` is rarely a node-path typing source; this is mostly for completeness and for the UID/ext_resource graph.)

---

## Workstream 2 — Script↔scene association

To type `$Path` inside `main_menu.gd`, we must know **which scene owns `main_menu.gd`**. The link is the scene's node `script = ExtResource("id")` where that ext_resource's `path` is `main_menu.gd`.

### 2.1 The resolution

```rust
// crates/gdscript-db (Phase-3 project model) — the project-wide reverse index.
// In Phase 3 this is a salsa query, invalidated when any .tscn's ext_resources change.
pub struct ScriptSceneIndex {
    /// .gd FileId -> the scenes that attach it (and at which node).
    by_script: FxHashMap<FileId, Vec<SceneAttachment>>,
}
pub struct SceneAttachment { pub scene: FileId, pub node: NodeIdx }  // the scene + the script-owning node

/// Resolve: which scene(s) own this script, and where is the script's "root" node?
/// The node-path base for `$X` inside script S is the node that ATTACHES S (its `$` is relative to it).
pub fn owning_scenes(db: &dyn Db, script: FileId) -> &[SceneAttachment];
```

Built by scanning every `.tscn`'s `ext_resources` for `type="Script"` entries and mapping their `path` (→ FileId via the `res://` mapper) to the scenes that reference them via a node's `script =`. This is nearly free in Phase 3 because we already parse every scene for the project graph.

**The node-path base matters:** `$X` inside script `S` is resolved **relative to the node that attaches `S`** (usually the scene root, but a script can be attached to a non-root node). So association returns *both* the scene *and* the attaching node; the path walk (Workstream 3) starts there, not blindly at the root.

### 2.2 The 1-script-many-scenes ambiguity policy

A single `.gd` can be attached to **multiple scenes** (a reusable component script; a base UI controller). Then `$StartButton` might be `Button` in one scene and `TextureButton` in another. Policy ([`research/09`](research/09-type-system-and-inference.md) §5.2 — "type as the common base or annotate ambiguity"):

| Policy | Behavior | When |
|---|---|---|
| **Single** (slice default) | Exactly one owning scene → use it directly. | The overwhelmingly common case; the only one the slice handles. |
| **Common-base union** (full default) | N owning scenes → for each node path, resolve the type in each scene and take the **common base type** (walk the inheritance table to the nearest shared ancestor; e.g. `Button` ∪ `TextureButton` → `BaseButton`). Never *wrong*, just less specific. | Default for the full phase. Degrades gracefully toward `Node`. |
| **Primary** (configurable) | Pick a designated primary scene (heuristic: the scene whose root attaches the script; or the only scene where the script is on the root; or a user setting). Use its types directly; note the ambiguity in hover. | Opt-in via project config; better specificity when the user knows the canonical scene. |

**Where it lives:** the index and policy live in the **Phase-3 project model** (`gdscript-db`). The **slice** sidesteps this entirely: it takes a **single explicitly-associated scene** (one owning scene, or an explicit "type this `.gd` against this `.tscn`" association pushed via `apply_change`), so ambiguity is out of scope until the full phase.

---

## Workstream 3 — Node-path typing in the checker (`gdscript-hir`)

This wires the `SceneModel` into the Phase-2 inference walk. The Phase-2 checker already has the seam: `Expr::GetNode { path }` and `Expr::Path`/`$`-sugar lower to a `GetNode` expr that currently returns `Ty::Object(Node)` ([`PHASE-2`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) §4.1: `Expr::GetNode { .. } => Ty::Object(Node) // Phase 4 -> concrete`). **Phase 4 replaces that one rule.**

### 3.1 What lowers to a node-path query

From [`research/04`](research/04-gdscript-semantics-and-features.md) §1.9, all of these are the same operation:

| Source syntax | Lowered | Path |
|---|---|---|
| `$Panel/VBox/Button` | `GetNode{ path: "Panel/VBox/Button", kind: Path }` | literal node path |
| `$"Panel/My Button"` | `GetNode{ path: "Panel/My Button", kind: Path }` | quoted (spaces) |
| `%StartButton` | `GetNode{ path: "StartButton", kind: Unique }` | unique-name lookup |
| `get_node("Panel/VBox/Button")` | `GetNode{ path: "...", kind: Path }` | string-literal arg only |
| `get_node(^"Panel/VBox/Button")` | `GetNode{ path: "...", kind: Path }` | NodePath literal arg |
| `get_node_or_null("...")` | `GetNode{ path, kind: Path, nullable: true }` | result type is the node `or null` (still type the node) |
| `@onready var x = $Panel/VBox/Button` | the `var`'s init is the `GetNode` expr | type the decl from the resolved node ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.4: "@onready defers *assignment* but the type is the resolved node's — resolve at decl site") |
| `get_node(some_var)` / computed path | `GetNode{ path: None }` | **non-literal → unknowable statically → `Node`** (never guess) |

### 3.2 The resolution function

```rust
// crates/gdscript-hir/src/infer.rs  — replaces the Phase-2 `Expr::GetNode => Node` stub.

fn infer_get_node(&mut self, path: Option<&str>, kind: NodePathKind, e: ExprId) -> Ty {
    let node_ty = self.resolve_node_path(path, kind);
    match node_ty {
        Some(ty) => ty,                        // concrete: Button, CharacterBody2D, the attached script type...
        None     => self.node_ty(),            // fall back to Ty::Object(Node) — NEVER regress
    }
}

/// Returns the node's type, or None to fall back to `Node`. The heart of the killer feature.
fn resolve_node_path(&mut self, path: Option<&str>, kind: NodePathKind) -> Option<Ty> {
    let path = path?;                                   // non-literal/computed path -> None -> Node
    // 1. Which scene(s) own this script, and from which node does `$` resolve?
    let attach = self.owning_attachment()?;            // slice: the single associated scene + attach node
    let scene = self.scene_model(attach.scene)?;       // Arc<SceneModel> (cached in Phase 3)

    // 2. Find the target node in the tree.
    let node_idx = match kind {
        NodePathKind::Unique => scene.resolve_unique(path)?,                 // %Name -> unique_name_in_owner
        NodePathKind::Path   => scene.resolve_path_from(attach.node, path)?, // walk from the script's node
    };
    let node = scene.node(node_idx);

    // 3. The node's base type: direct `type=`, else instanced sub-scene root type, else Node.
    let base_ty: Ty = if let Some(t) = &node.decl_type {
        self.api.class_by_name(t).map(Ty::Object).unwrap_or(self.node_ty())   // "Button" -> Object(Button)
    } else if let Some(inst) = &node.instance {
        self.instanced_root_ty(scene, inst).unwrap_or(self.node_ty())         // sub-scene root (hard tail)
    } else {
        self.node_ty()                                                        // shouldn't happen; be safe
    };

    // 4. Refine by the node's ATTACHED SCRIPT, if any: a script's class is a subtype of its native base,
    //    so the script type is MORE specific. (Needs the project graph -> full phase; slice: base_ty only.)
    if let Some(script_id) = &node.script {
        if let Some(script_ty) = self.script_class_ty(scene, script_id) {     // resolve_external in Phase 3
            return Some(script_ty);   // e.g. node type="Button" + script=Fancy.gd(class_name Fancy) -> Fancy
        }
    }
    Some(base_ty)
}
```

The resolution order, stated plainly (mirrors [`research/09`](research/09-type-system-and-inference.md) §5.2):

1. **Parse the path** → segments (or `None` for non-literal → `Node`).
2. **Walk the owning scene's `NodeTree`** from the script's attaching node (for `%Unique`, search `unique_name_in_owner` nodes in the owner subtree).
3. **Read the node's type:** direct `type=` (a native class → `EngineApi`), else the **instanced sub-scene's root type** (recursive — hard tail), else `Node`.
4. **Refine by attached script:** if the node has `script = ExtResource(...)`, the script's `class_name`/script type is a subtype → use it (it's strictly more specific).
5. **Fall back to `Node`** at any failure. Never produce a *wrong* type; the floor is the engine's own `Node`.

Once `resolve_node_path` returns `Ty::Object(Button)`, **everything downstream is free**: `start_button.text` resolves through the exact same inheritance-table member lookup Phase 2 already built (`Button → BaseButton → Control → ...`), completion lists `Button`'s members, `start_button.pressed.connect(...)` checks against the real `Signal`. Scene awareness is *just a better source type* feeding the Phase-2 machine.

### 3.3 `%Unique` resolution detail

`%Name` ≡ `get_node("%Name")` resolves to the node with `unique_name_in_owner = true` within the **current scene's owner** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.4). Slice: search `unique_nodes` scene-wide (single owner = the scene). Hard tail: inside an instanced sub-scene, `%` resolves within *that sub-scene's* owner, so the search is scoped to the owner subtree, not the whole composed tree — this needs the sub-scene recursion model.

---

## Workstream 4 — Scene-aware diagnostics & features

Every feature below is a pure `(db, FilePosition|FileId) -> POD` fn on `Analysis` ([`01`](01-ARCHITECTURE.md) §2), built from the `SceneModel` + the resolved node type. POD lives in `gdscript-base`.

### 4.1 Feature → scene data needed → POD

| Feature | `Analysis` method | Scene data needed | POD result |
|---|---|---|---|
| **Invalid node-path warning** | `diagnostics(file)` | path walk over owning `SceneModel` returns `None` for a **literal** path | `Diagnostic { code: "INVALID_NODE_PATH", severity: Warning, range: $-expr span, message }` |
| **Resolved-type hover on `$Path`** | `hover(pos)` | `resolve_node_path` → `Ty`; node's `header_span` for the source | `HoverResult { ty_label: "Button", doc: "from MainMenu.tscn → Panel/VBox/StartButton", range }` |
| **Node-path completion inside `$`/`get_node`** | `completions(pos)` | the prefix's resolved node → `children_of` (actual child node names!) | `Vec<CompletionItem>` (label = child name, detail = its `type=`, kind = Field/Value) |
| **Go-to-definition `$Path` → `.tscn` node** | `goto_definition(pos)` | target `NodeIdx.header_span` in the scene `FileId` | `Vec<NavTarget { file: scene, range: header_span }>` |
| **Wrong-type usage** (rides Phase-2) | `diagnostics(file)` | resolved concrete type → existing `UNSAFE_PROPERTY/METHOD_ACCESS` now fire with the *real* type | `Diagnostic` (Phase-2 codes, now sharper — typo `.txet` on a known `Button` flags) |
| **Member completion after `$Path.`** (rides Phase-2) | `completions(pos)` | resolved node `Ty` → inheritance-table member set | `Vec<CompletionItem>` — `Button`'s members, not `Node`'s |

### 4.2 The node-path-completion POD (the standout)

Typing `$Panel/` or `get_node("Panel/` and getting the **real child node names from the scene** is something no other GDScript tool offers. Sketch:

```rust
// crates/gdscript-ide/src/scene_completion.rs
pub struct NodePathCompletion {
    pub items: Vec<NodePathItem>,
    pub replace_range: TextRange,         // the path segment under the cursor (byte range to overwrite)
}
pub struct NodePathItem {
    pub label: String,                    // "VBox"
    pub node_type: String,                // "VBoxContainer"  (shown as detail)
    pub has_children: bool,               // offer to keep completing deeper ("VBox/")
    pub unique: bool,                     // true => also offerable as "%VBox"
    pub kind: CompletionItemKind,         // Field
}

/// Completion entry point: cursor is inside a $..., %..., or get_node("...") string.
pub fn complete_node_path(db: &dyn Db, pos: FilePosition) -> Option<NodePathCompletion> {
    let (script, prefix, kind, range) = parse_node_path_context(db, pos)?;  // split "Panel/" -> ["Panel"]
    let attach = owning_attachment(db, script)?;
    let scene = scene_model(db, attach.scene)?;
    let base = match kind {
        NodePathKind::Unique => return Some(unique_name_completions(&scene, range)), // all %-targets
        NodePathKind::Path   => scene.resolve_path_from(attach.node, prefix)?,       // node at the prefix
    };
    let items = scene.children_of(Some(base)).map(|child| NodePathItem {
        label: child.name.to_string(),
        node_type: child.decl_type.clone().unwrap_or_else(|| "PackedScene".into()).to_string(),
        has_children: scene.children_of(Some(child.idx)).next().is_some(),
        unique: child.unique_name_in_owner,
        kind: CompletionItemKind::Field,
    }).collect();
    Some(NodePathCompletion { items, replace_range: range })
}
```

---

## Workstream 5 — The early "wow-demo" slice (optional accelerator)

The minimal cut that can land **right after Phase 2**, before Phase 3's project graph, for an outsized demo ([`research/09`](research/09-type-system-and-inference.md) §3.4 note, §7; [`ROADMAP.md`](ROADMAP.md) Tier→Phase). Exactly what to build:

**In scope (the minimal cut):**
- `gdscript-scene` Workstream-1 parser, but **only**: scene header, `ext_resource type="Script"`, `[node name= type= parent=]`, body `script =` and `unique_name_in_owner`. (Skip `instance=`, sub_resources, connections.)
- **Single owning scene**, explicitly associated — pushed via `apply_change` as "this `.gd` is owned by this `.tscn`" (or the only `.tscn` in the same directory that attaches it). **No project scan, no `ScriptSceneIndex`, no UID map, no ambiguity policy.**
- **Direct `type=` nodes only** — no instanced-sub-scene recursion (an `instance=` node → `Node`), no attached-script refinement (a node `script=` → still its `type=`, not the script class — that needs the project graph).
- Path forms: `$A/B/C`, `%Name`, `get_node("A/B/C")` literal. Computed paths → `Node`.
- Two features: **hover** on `$Path` shows the resolved type, and **member completion** after `$Path.` lists the real members. (Node-path *segment* completion is a small add; include if cheap.)

**Out of scope for the slice (deferred to the full phase):** instanced sub-scenes, `uid://`, multi-scene ambiguity, attached-script refinement, invalid-path diagnostics across the project, salsa caching (the slice re-parses the one scene per analysis — fine, scenes are small).

**The demo:** open `main_menu.gd` + `MainMenu.tscn`, hover `$Panel/VBox/StartButton` → **`Button`**; type `start_button.` → `Button` members. Side-by-side with the Godot editor showing `Node`. That single screenshot is the project's headline.

---

## Testing strategy

1. **Real-`.tscn` corpus** — vendor a set of real scenes from **Godot demo projects** (`godot-demo-projects`: `dodge_the_creeps`, UI demos; the `Maaack/Godot-Game-Template` menus already referenced in [`research/04`](research/04-gdscript-semantics-and-features.md) fixtures). Assert the parser produces zero panics, a sane node tree, and round-trips structure (node count, root, parent links) on **every** scene. This is the robustness gate (mirrors Phase-1's "parse the corpus with zero panics").
2. **Node-path typing goldens** (`fixtures/scene/<case>/`): a `.gd` + its `.tscn` + an `*.expected` listing, per `$`/`%`/`get_node` expr, the expected resolved `Ty`. Harness associates the scene, runs inference, snapshots `display(expr_ty)` at each node-path expr. The canonical case: `$Panel/VBox/StartButton → Button`; `%StartButton → Button`; `start_button.text → String`.
3. **Invalid-path cases** — `$DoesNotExist`, `$Panel/Wrong`, `get_node("typo")` → assert an `INVALID_NODE_PATH` diagnostic at the right span; assert a *computed* `get_node(var)` does **not** warn (unknowable ≠ invalid) and types to `Node`.
4. **Sub-scene recursion cases** (full phase) — a parent scene instancing `enemy.tscn`; assert `$Enemy` resolves to `enemy.tscn`'s root type, and `$Enemy/Sprite` recurses into the sub-scene. Include a 2-level nesting and a **cycle** (A instances B instances A) → assert bounded depth, no infinite loop, degrade to `Node`.
5. **Ambiguity cases** (full phase) — one `.gd` attached to two scenes with `Button` vs `TextureButton` at the same path → assert **common-base** policy yields `BaseButton`; assert "primary" config yields the primary's type.
6. **Attached-script refinement** — node `type="Button"` + `script = ExtResource(Fancy.gd)` where `Fancy` has `class_name Fancy extends Button` → assert `$That → Fancy` (more specific than `Button`).
7. **Completion snapshots** — cursor inside `$Panel/|` → assert the item set = the real children (`{VBox, ...}`) with their `type=` as detail; `%|` → assert all unique-name targets.
8. **Go-to-def** — `$Panel/VBox/StartButton` → assert the `NavTarget` points at the `[node name="StartButton" ...]` header span in the `.tscn` `FileId`.
9. **Degradation / never-regress** — an unparseable/binary/missing scene → assert every `$Path` types to `Node` with **no** spurious diagnostics (parity floor, never worse than Phase 2).
10. **wasm32 CI gate** — `cargo check -p gdscript-scene --target wasm32-unknown-unknown` green; a tiny wasm smoke parses a `.tscn` string and resolves one path.
11. **Perf** (`criterion`) — parse a representative ~200-node scene **< 5 ms**; a node-path resolution (warm, scene cached) **< 1 ms**. Scene parsing is cached (Phase 3) so it's off the per-keystroke path.

---

## Exit criteria (testable — mirrors ROADMAP Phase 4)

All must pass on a `.gd` + its associated `.tscn`:

- [ ] **`$Panel/VBox/StartButton` infers `Button`** (not `Node`) with **zero annotations** — and `start_button.text` resolves to `String`, `start_button.pressed` to a `Signal`, via the existing inheritance-table lookup.
- [ ] **`%StartButton` infers `Button`** via `unique_name_in_owner` lookup.
- [ ] **`@onready var x = $Path`** types `x` from the resolved node at the decl site.
- [ ] **`get_node("Panel/VBox/StartButton")`** (string literal) types identically to `$Panel/VBox/StartButton`; a **computed** `get_node(var)` types to `Node` with **no** false warning.
- [ ] **Invalid path warns** — `$DoesNotExist` produces an `INVALID_NODE_PATH` diagnostic at the path span.
- [ ] **Attached-script types refine** — a node with `type="Button"` + `script=Fancy.gd (class_name Fancy)` resolves `$That` to `Fancy`.
- [ ] **Instanced sub-scene** — `$Enemy` (an `instance=ExtResource("enemy.tscn")` node) resolves to `enemy.tscn`'s root type (full phase).
- [ ] **Node-path completion** — inside `$Panel/` the analyzer offers the real child node names from the scene.
- [ ] **Go-to-definition** — `$Path` navigates to the `[node ...]` line in the `.tscn`.
- [ ] **Never regresses** — any unresolvable scene/path falls back to `Node` with no spurious diagnostics.
- [ ] **wasm32 CI** for `gdscript-scene` is green.

This is intelligence the Godot editor LSP cannot produce ([`research/09`](research/09-type-system-and-inference.md) §2.4, §5).

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| **`.tscn` format drift across Godot versions** (4.x `format=3`; 3.x `format=2`; a future `format=4`; new node attributes) | Parse defensively — the node grammar (`name`/`type`/`parent`/`instance`/`script`) is stable across 4.x; record `format` and treat unknown attributes as ignorable. Target `format=3`; older/newer → best-effort then degrade to `Node`. A corpus test (Testing #1) across vendored Godot versions catches drift early. |
| **Sub-scene recursion depth / cycles** (A instances B instances A; deep nesting) | A **visited-set + depth cap** in `instanced_root_ty`; on cycle/limit, stop and return `Node`. Cached per scene (salsa), so recursion is memoized not re-walked. Test #4 includes an explicit cycle. |
| **1-script-many-scenes ambiguity** (different types at the same path) | **Common-base union by default** (never *wrong*, only less specific); **primary** scene configurable; **single** in the slice. Hover notes ambiguity. The floor is always `Node`. |
| **`uid://` indirection** (ext_resource by uid, no path) | Resolve via the Phase-3 **UID map**, rebuilt from authoritative `uid="..."` strings (not the stale `.godot/uid_cache.bin` — [`research/04`](research/04-gdscript-semantics-and-features.md) §3.7); slice prefers `path=` and defers uid-only refs to `Node`. |
| **Performance of scene parsing** (large scenes; re-parse per keystroke) | Cache `scene_model(db, FileId)` as a **salsa query** invalidated only when that `.tscn` changes (Phase 3); editing a `.gd` never re-parses its scene. Parser is single-pass line-oriented (Testing #11: < 5 ms / ~200 nodes). The slice (no salsa) re-parses one small scene — acceptable. |
| **Stale scene cache** (the `.tscn` changed but typing is stale) | Treat `.tscn` as a first-class VFS input pushed via `apply_change` (a file watcher in the LSP client feeds edits, same as `.gd`); salsa invalidation re-runs only the affected node-path queries. |
| **Keeping it optional so it never regresses to worse-than-`Node`** | **Every** failure path returns `Ty::Object(Node)` — unparseable scene, missing association, computed path, missing file, ambiguity collapse, recursion limit. The feature is **strictly additive**: at worst we match the engine's `Node`-everywhere baseline; we are never *wrong*. A dedicated test (#9) asserts this. |
| **Script attached to a non-root node** (so `$` is relative to a non-root) | Association returns the **attaching node**, and the path walk starts there (Workstream 2.1, 3.2) — not blindly at the scene root. Tested with a script-on-child fixture. |

---

## References (relative links)

- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — crate stack (§1, the `gdscript-scene` crate row), `AnalysisHost`/`Analysis` API (§2), data model (§5), portability rules (§7, the wasm gate `gdscript-scene` must pass).
- [`ROADMAP.md`](ROADMAP.md) — Phase 4 deliverable + exit criteria; the Tier→Phase mapping ("Tier 3 slice", "early scene slice possible"); the value-earliest ordering that pulls the slice forward.
- [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) — the type layer this phase sharpens: `Ty`, the `EngineApi` inheritance table, and the `Expr::GetNode => Node` stub Phase 4 replaces.
- [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) — the project model the full phase needs: the workspace scan, `res://`/UID maps, `class_name` registry, salsa caching, and the home of `ScriptSceneIndex`.
- [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) — the LSP/guitkx/playground clients that surface scene-typed hover, completion, and go-to-def to users.
- [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) — the full Tier-3 tail beyond typing: `@onready`/lifecycle flow validation, scene-aware rename, the complete warning set.
- [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) — **PRIMARY**: §5 (scene awareness — the killer feature; the `.tscn` format; typing a node path), §2.4 (`$`/`%`/`get_node` node-path typing), §3.4/§7 (the 90% slice + the pull-forward recommendation).
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — **PRIMARY**: §1.9 (`$ % &"" ^"" get_node`), §1.10 (`@onready`), §3.4 (the per-`.tscn` extraction model), §3.5 (`.tres`), §3.7 (authoritative vs derived files; the UID map), §5 (the engine LSP gaps this phase exploits).
- [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md) — the `extension_api.json` engine model that maps a node's `type="..."` to a real class with members.
