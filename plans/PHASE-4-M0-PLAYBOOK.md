# gdscript-analyzer Phase 4 — M0 Implementation Playbook: the `gdscript-scene` crate

> **Provenance:** produced by a fact-checked research workflow (5 research agents → 3-vote adversarial verification of every load-bearing format claim → synthesis), grounded in Godot source (`resource_format_text.cpp`, `variant_parser.cpp`) + real `.tscn` files. 31 confirmed facts, 1 refuted, 41 plan corrections.
>
> **⚠️ STRING-TYPE CORRECTION (verified against the workspace):** this crate uses **`smol_str::SmolStr`** for stored strings and **`rustc_hash::FxHashMap`** — the actual workspace conventions. The playbook text below says `EcoString`/`triomphe` in places (a synthesis hallucination, flagged by the research's own open-question #27 and confirmed false: `grep ecow|EcoString|triomphe` is empty). **Read every `EcoString` as `SmolStr`.**

> **Status:** execution playbook (settled — decisions made, not options). **Tier:** 3 (slice). **Reconciles:** [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) §1 (Workstream 1, the parser) with a primary-source pass against Godot `scene/resources/resource_format_text.cpp` + `core/variant/variant_parser.cpp` (4.3/4.4/master), the official TSCN doc, and a real-corpus read (`ReactiveUI-Gadot/examples/main.tscn`, godot-demo-projects `dodge_the_creeps`, the engine's own `modules/gdscript/tests/.../get_node/get_node.tscn`).
> **What M0 is:** the pure, wasm-clean `fn(&str) -> SceneModel` text parser for `.tscn`/`.tres` with byte spans — the foundation everything in Phase 4 stands on. **No** type resolution, **no** project index, **no** salsa. M0 ships a parser and a node tree; M1+ wires it into `gdscript-hir` typing.
> **The one rule, unchanged from the plan:** the parser is **strictly additive and never fails**. Every unparseable/binary/unknown form degrades to "treat node as `Node`" via an empty-or-partial `SceneModel` + a `SceneProblem` — never a panic, never an `Err`. The floor is parity with the engine's `Node`-everywhere baseline.

---

## 0. Where the plan was right, wrong, and silent (the corrections that drive this playbook)

The plan's §1 design **holds**. The dossier confirms the two-pass approach, the `SceneModel`/`SceneNode` shape, the `ExtId(String)` decision, the parent-path semantics, and the degrade-never-fail rule. But the primary-source pass forced **concrete corrections and additions** the implementer MUST honor:

| # | Plan said / was silent | Corrected fact (load-bearing) | Source |
|---|---|---|---|
| C1 | Edge table (§1.4): `groups=PackedStringArray(...)` | **WRONG.** `groups` is a bracket array literal `groups=["a", "b"]` (single-line since PR #52284). `node_paths` is the one that's `PackedStringArray("prop")`. | resource_format_text.cpp writer; godot PR #52284 |
| C2 | Header attribute values implicitly quoted | **WRONG for modern files.** Header values can be **bare integers** (`unique_id=1975992027`), **bools**, **`[...]` arrays** (`groups`), and **`Type(args)` constructors** (`node_paths=PackedStringArray(...)`, `instance=ExtResource("id")`). A lexer that assumes quoted values fails on **every** 4.6+ node header. | dodge_the_creeps/main.tscn (master); variant_parser.cpp |
| C3 | Plan's example shows `unique_name_in_owner=true` **inside the `[node ...]` header** (line 155) | **WRONG.** `unique_name_in_owner` is a **body property line** (`unique_name_in_owner = true`), never a header attribute. So is `script =`. The header carries the (unrelated) integer `unique_id=N`. **Never conflate `unique_id` (header int) with `unique_name_in_owner` (body bool).** | resource_format_text.cpp (header reads name/parent/unique_id/type/node_paths/instance/instance_placeholder/owner/index/groups only); get_node.tscn fixture |
| C4 | "engine LSP **architecturally cannot** do this" (§ Why this matters, line 108) | **OVERSTATED.** The engine *does* type `$`/`%` from the open scene — but **completion-only**, inside `#ifdef TOOLS_ENABLED`, **only with a running editor**, and it does **not** flow a persistent `Ty` into inference/hover/diagnostics. Our edge is **standalone/headless type-flow**, not that the engine is blind. Soften the claim in the prose plan. | godot `modules/gdscript/tests/scripts/completion/get_node/` |
| C5 | Header tags: 5 named (`gd_scene, ext_resource, sub_resource, node, connection`) | **INCOMPLETE.** The engine's loader recognizes **8**: add `gd_resource`, `resource`, `editable`. Unknown tags are `ERR_FILE_CORRUPT` in the engine — **we degrade instead.** | resource_format_text.cpp |
| C6 | `format=3` ⇒ exactly Godot 4.x | **REFINE.** On-disk files are `format=3` (`FORMAT_VERSION_COMPAT`); `format=4` is the internal max, never written by default. Treat **`format>=3` as the 4.x family**; do not branch on literal `3`. `format=2` is 3.x. | resource_format_text.cpp `#define FORMAT_VERSION 4 / _COMPAT 3` |
| C7 | `load_steps` listed as a header field to read | **DEPRECATED.** Optional, omitted when ≤1 step, **fully removed in 4.6+ files**. Parse-if-present, **never require, never depend on its value.** | resource_format_text.cpp; master docs |
| C8 | Silent on `script_class=` | **ADD.** `gd_scene` / `gd_resource` headers can carry `script_class="ClassName"` — a free shortcut to the root/resource's `class_name` script type **without resolving the script file**. Record it. | resource_format_text.cpp; real `.tres` |
| C9 | Silent on instanced **root** (inherited scene) | **ADD.** A root node with `instance=` and **no `parent=`** is an **inherited scene** (`set_base_scene`), distinct from a child instance. Record the distinction as data (resolve later). | resource_format_text.cpp lines 219-256 |
| C10 | `^"..."` NodePath listed in §1.4 / value forms | **CLARIFY.** `^"..."` is **GDScript source syntax**, NOT a `.tscn` value form. In `.tscn` value bodies, NodePaths serialize as `NodePath("...")`. The `^` form belongs to M1's `$`-path lexer, not M0. | variant_parser.cpp (no `^` case) |
| C11 | `#` not addressed | **ADD a hard rule.** Inside value position `#` is a **Color literal** (`#RRGGBBAA`), **not** a comment. `;` is the only comment char (and only at line/value-lexer start, outside quotes). | variant_parser.cpp |
| C12 | Strings: escapes only | **ADD.** Quoted strings may contain **literal embedded newlines** (e.g. `text = "Dodge the\nCreeps"` written as a real 2-line string). The value-skipper must stay inside the quote across physical line breaks. No triple-quoted strings exist. | hud.tscn; variant_parser.cpp |

These are folded into the spec below. **C1, C2, C3, C11, C12 are the five that break the parser if gotten wrong** — they are the adversarial-verify targets for the test plan (§8).

---

## 1. Scope of M0 (what ships vs what defers)

### M0 ships — the 90% slice, parser-only

`gdscript-scene` as a new **core crate** (`crates/gdscript-scene/`), wasm32-clean, with exactly:

1. **`pub fn parse_scene(text: &str) -> SceneModel`** — the pure entry point. No `FileId`, no db, no fs. (A thin `parse_scene_with(text, kind_hint)` may exist for `.tres` but kind is also auto-detected from the header tag.)
2. The **`SceneModel`/`SceneNode`/`ExtResource`/`SceneProblem`** data model (§3) with **byte spans on every node header, every ext_resource, and the scene header**.
3. **Two-pass parse** (§4): sectionize with a Variant-aware header value lexer + lossless body value-skipper; then build the tree (parent resolution, `by_path`, `unique_nodes`).
4. **Binary `.scn`/`.res` detect-and-degrade** (magic `RSRC`/`RSCC`) → empty model + `SceneProblem::BinaryResource`.
5. The **read-only public API surface** (§3.3): `resolve_path`, `resolve_path_from`, `resolve_unique`, `node_with_script`, `children_of`, `node`.
6. **`.tres` support** (`SceneKind::Resource`): same sectionizer, `[resource]` section recognized, `script_class=` extracted, **no `[node]` tree**.
7. The **recorded-but-not-resolved typing data** (§5): `decl_type`, `script` ExtId, `instance` ExtId, `instance_is_inherited_root` flag, `unique_name_in_owner`, `script_class`.
8. Unit + real-corpus tests (§8) and the **wasm32 CI gate**.

### M0 explicitly defers (to M1+)

| Deferred | Milestone | Why it's not M0 |
|---|---|---|
| Mapping `decl_type`/`script`/`instance` to a `Ty` | **M1** (`gdscript-hir` `resolve_node_path`) | Needs the Phase-2 `EngineApi` + Phase-3 `class_name` registry. M0 only **records** the strings/ids. |
| Instanced sub-scene **recursion** (follow `instance=` → another `.tscn`'s root type) | M1+ (hard tail) | Needs cross-file VFS + the project graph. M0 records `instance: Option<ExtId>` + `instance_is_inherited_root: bool` and stops. |
| Project-wide **script→scene reverse index** (`ScriptSceneIndex`) | M1 (lives in `gdscript-db`) | M0's `node_with_script(path)` answers the *per-scene* half only; the cross-project map is Phase-3 territory. |
| **salsa caching** (`scene_model(db, FileId)`) | M1 | M0 is a pure fn; caching is added by wrapping it in a tracked query. The pure fn must stay pure (no db dependency) so it remains the cache body. |
| **`uid://` resolution** (uid-only ext_resource → path) | M1+ | M0 records `uid: Option`; resolution needs the project UID map. Prefer `path=` when present. |
| **1-script-many-scenes** policy (union/primary) | M1 | A per-scene parser has no cross-scene view. |
| IDE features (hover/completion/goto/diagnostics) | M1+ (`gdscript-ide`) | They consume the resolved `Ty`, which M0 doesn't produce. M0 provides the spans + tree they'll need. |
| Binary `.scn` **parsing** | v1+ | Detect-and-degrade only. |
| Resource **value** deserialization (`sub_resource` bodies, `.tres` data) | never (non-goal) | We need types/structure, not values. Skip losslessly. |

---

## 2. The `.tscn`/`.tres` format spec we implement (CONFIRMED facts only)

### 2.1 File shape

An INI-like sequence of **sections**. Each section = one bracketed **header line** `[<tag> key=value …]` optionally followed by `key = value` **body property lines** until the next `[` at line-start (outside any open value) or EOF. The header is **always a single physical line** (never wrapped).

**The 8 header tags the engine recognizes** (trust the SOURCE, not the doc which omits 3):
`gd_scene`, `gd_resource`, `ext_resource`, `sub_resource`, `resource`, `node`, `connection`, `editable`.
→ An **unknown tag** is `ERR_FILE_CORRUPT` in the engine; **we record `SceneProblem::UnknownTag` and skip the section** (degrade, never error).

### 2.2 Header descriptors

**Scene:** `[gd_scene load_steps=N format=3 uid="uid://..." script_class="..."]`
**Resource (`.tres`):** `[gd_resource type="X" script_class="Name" load_steps=N format=3 uid="uid://..."]`

| Field | Required? | We read | Notes |
|---|---|---|---|
| `format=` | required-in-practice | `format: u8` | **Only reliable version field.** `>=3` ⇒ 4.x family (C6); `2` ⇒ 3.x. |
| `uid=` | optional | `uid: Option<EcoString>` | Form `uid="uid://<base62>"`. Absent in many files (local `main.tscn` has none). |
| `load_steps=` | optional/**deprecated** | parse-if-present, **ignore value** (C7) | Absent in 4.6+. Never require. |
| `script_class=` | optional | `script_class: Option<EcoString>` | **NEW (C8).** Names the root/resource's `class_name` script directly. Cheap type refinement for M1. |
| `type=` (gd_resource only) | required for `.tres` | `resource_type: Option<EcoString>` | The resource's own class (e.g. `"ArrayMesh"`, `"Resource"`). |

### 2.3 `ext_resource`

`[ext_resource type="..." uid="uid://..." path="res://..." id="..."]` — writer order is **type, uid?, path, id**, but **parse order-independently** (3.x put `path` before `type`).

- **Required:** `type`, `path`, `id` (engine errors if missing — **we record `SceneProblem` and keep going**).
- **Optional:** `uid`.
- **`id` is an opaque QUOTED STRING** — `"1"`, `"4"`, `"1_app"`, `"3_veqnc"`, `"StyleBoxFlat_xx12y"`. **Never parse as int.** (3.x legacy: bare-int `id=1` — accept and store as the string `"1"`.)
- **`type="Script"`** = script-attach discriminator. **`type="PackedScene"`** = sub-scene discriminator. (Recorded as data; M1 branches on it.)
- Engine precedence when both `uid` and `path` present is **uid-first**; for M0 **prefer `path`** (record both, defer uid-only refs).

### 2.4 `sub_resource`

`[sub_resource type="..." id="..."]` — **type and id only** (no uid, no path). `id` is a quoted string (`"1"` or `"SphereShape3D_tj6p1"`). **Record `id → type`; skip the body losslessly** (bodies are multiline dicts/`Packed*Array`/constructors).

### 2.5 `node`

**Header attributes the engine reads (exact set), all optional except `name`:**
`name`, `parent`, `unique_id`, `type`, `node_paths`, `instance`, `instance_placeholder`, `owner`, `index`, `groups` (+ internal `parent_id_path`, `owner_uid_path`).
**Parse order-independently and ignore unknown keys.** (Real files interleave `unique_id` between `parent` and `instance`.)

| Header attr | We read into | Value form (LOAD-BEARING for the lexer) |
|---|---|---|
| `name` | `name: EcoString` | quoted, **c-escaped**; may contain **spaces** (`name="My Button"`) and escapes |
| `type` | `decl_type: Option<EcoString>` | quoted string; **may be a custom `class_name`** (`type="Player"`), not only a native class (C: M1 must consult the class_name registry, not just `EngineApi`) |
| `parent` | `parent_path: Option<EcoString>` | quoted; `"."`=child of root; `"Panel/VBox"`=relative, **root name excluded**; **absent ⇒ root** |
| `instance` | `instance: Option<ExtId>` | **constructor** `ExtResource("id")` — unquoted value with parens (C2) |
| `instance_placeholder` | `instance: Option<ExtId>` + `placeholder: bool` | quoted `"res://..."` (lazy-load variant) |
| `unique_id` | **ignored** | **bare integer** `1975992027` (4.6+); NOT `unique_name_in_owner` (C3) |
| `index` | ignored (or record) | **quoted** string `index="0"` |
| `groups` | ignored (or record) | **bracket array** `["a","b"]` (C1) |
| `node_paths` | ignored | **constructor** `PackedStringArray("p1","p2")` (C2) |
| `owner` | ignored | quoted NodePath |

**Body property lines we read** (everything else skipped):
- `script = ExtResource("id")` → `script: Option<ExtId>`
- `unique_name_in_owner = true` → `unique_name_in_owner: bool` (C3 — body, not header; exact spelling)

**Root rule:** exactly one node has **no `parent=`**; it is always the first `[node]`. If a node has no `type=`, it is **instanced** (engine `TYPE_INSTANTIATED`); type comes from the referenced `PackedScene`. If that instanced node is also the root (no `parent=` + `instance=`), it is an **inherited scene** → record `instance_is_inherited_root = true`.

### 2.6 `connection`, `editable`, `resource`

- `[connection signal="..." from="..." to="..." method="..." flags=N? unbinds=N? binds=[...]?]` — **parse-and-ignore** for M0 (keep the section spans for future signal go-to). `from`/`to` are node paths, `"."`=root. (Note writer quirk `binds= [...]` with a space — harmless to a tolerant lexer.)
- `[editable path="..."]` — sub-scene override marker; **parse-and-ignore**. Implies later `[node parent="Instanced/..."]` override lines that **add no typeable node** (base type comes from the sub-scene).
- `[resource]` (`.tres` only) — the main resource's own property body; **skip the body**, the type came from the `gd_resource` header.

### 2.7 Variant value forms the skipper must tolerate (CONFIRMED)

Quoted strings (`\b \t \n \f \r \" \\ \uXXXX \UXXXXXX`, **literal embedded newlines**, no triple-quotes); numbers (incl. negative/float/`inf`/`nan`); `true`/`false`/`null`; `Type(args)` constructors (`Vector2(...)`, `Color(...)`, `Transform3D(...)`, `ExtResource("id")`, `SubResource("id")`, `Resource(...)`, `Object(Class, k:v,...)`, `NodePath("...")`, `Packed*Array(...)`, **typed-array `Array[Node]([...])`**); arrays `[...]`; dicts `{...}` (often multiline, **trailing commas in 4.x**); `&"StringName"` and `@"StringName"` (deprecated); `#RRGGBBAA` **Color literal (NOT a comment, C11)**. Property **keys may contain `/`** (`surface_material_override/0 = ...`).

### 2.8 REFUTED / CONTESTED — what an implementer must avoid

- **REFUTED:** `groups=PackedStringArray(...)` — it is `groups=["a","b"]` (C1).
- **REFUTED:** `unique_name_in_owner` as a header attribute / equating it with `unique_id` (C3).
- **REFUTED:** `^"..."` as a `.tscn` value form (C10) — source-only.
- **REFUTED:** `#` as a comment (C11) — it's a Color literal.
- **REFUTED:** all header values are quoted (C2).
- **CONTESTED → DECIDED:** "engine LSP architecturally can't" (C4) — soften prose; it's completion-only + editor-bound, not blind.

---

## 3. The `SceneModel` / `NodeTree` Rust shape

Use the crate's existing conventions: `EcoString` for stored strings, `SmolStr` acceptable for short ids, `FxHashMap`, `triomphe::Arc` (when M1 wraps in salsa), `TextRange` from `gdscript-base`. **No external INI/serde-for-tscn dependency** — hand-written.

```rust
// crates/gdscript-scene/src/model.rs

/// One parsed .tscn or .tres. Produced by `parse_scene(&str)`. PURE — no FileId, no db.
/// In M1 this becomes the body of a salsa query `scene_model(db, FileId) -> Arc<SceneModel>`.
pub struct SceneModel {
    pub kind: SceneKind,                          // Scene | Resource
    pub format: Option<u8>,                       // 3 => 4.x family; 2 => 3.x; None => unknown
    pub uid: Option<EcoString>,                   // "uid://..."
    pub script_class: Option<EcoString>,          // gd_scene/gd_resource script_class= (C8)
    pub resource_type: Option<EcoString>,         // gd_resource type= (.tres only)

    pub ext_resources: FxHashMap<ExtId, ExtResource>,   // "1_app" -> {type, path?, uid?, span}
    pub sub_resources: FxHashMap<ExtId, SubResource>,   // id -> {type, span} (NO value body)

    pub nodes: Vec<SceneNode>,                    // flat arena; index = NodeIdx.0
    pub root: Option<NodeIdx>,                    // the (first) parent-less node

    /// Full name-path ("Panel/VBox/StartButton") -> NodeIdx. Built in pass 2. O(1) walks.
    pub by_path: FxHashMap<EcoString, NodeIdx>,
    /// unique_name_in_owner nodes: bare name ("StartButton") -> NodeIdx. Slice = scene-wide.
    pub unique_nodes: FxHashMap<EcoString, NodeIdx>,

    pub problems: Vec<SceneProblem>,              // non-fatal; the parser NEVER errors
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct NodeIdx(pub u32);
#[derive(Clone, PartialEq, Eq, Hash)]       pub struct ExtId(pub EcoString);  // opaque id string

pub enum SceneKind { Scene, Resource }

pub struct SceneNode {
    pub name: EcoString,                          // "StartButton" (unescaped, may contain spaces)
    pub decl_type: Option<EcoString>,             // type="Button" (None => instanced)
    pub parent_path: Option<EcoString>,           // raw "Panel/VBox" | "." | None(=root)
    pub parent_idx: Option<NodeIdx>,              // resolved in pass 2 (None => root)
    pub script: Option<ExtId>,                    // body: script = ExtResource("id")
    pub instance: Option<ExtId>,                  // header: instance=ExtResource("id") (sub-scene)
    pub instance_is_inherited_root: bool,         // root + instance= => inherited scene (C9)
    pub instance_placeholder: bool,               // instance_placeholder="res://..." variant
    pub unique_name_in_owner: bool,               // body: unique_name_in_owner = true (C3)
    pub header_span: TextRange,                   // byte span of the `[node ...]` line (go-to-def)
    pub name_span: TextRange,                     // byte span of the name="..." value (finer goto)
}

pub struct ExtResource {
    pub res_type: EcoString,                      // "Script" | "PackedScene" | "Texture2D" | ...
    pub path: Option<EcoString>,                  // "res://examples/app.gd"
    pub uid:  Option<EcoString>,                  // "uid://..." (M1 UID map)
    pub span: TextRange,                          // header line span
}

pub struct SubResource { pub res_type: EcoString, pub span: TextRange }

pub enum SceneProblem {
    BinaryResource,                               // RSRC/RSCC magic — detect & degrade
    UnknownTag       { at: TextRange },           // tag not in the 8 recognized
    MalformedHeader  { at: TextRange },           // couldn't lex the bracket header
    MissingExtField  { at: TextRange },           // ext_resource without type/path/id
    UnknownExtResource { id: ExtId, at: TextRange }, // script=/instance= points at missing id
    MultipleRoots    { roots: Vec<NodeIdx> },     // >1 parent-less node
    NoRoot,
    DanglingParent   { node: NodeIdx, parent_path: EcoString },
}
```

### 3.3 Public API surface (the only thing M1 calls)

```rust
pub fn parse_scene(text: &str) -> SceneModel;     // the entry point (pure, never panics)

impl SceneModel {
    pub fn node(&self, idx: NodeIdx) -> &SceneNode;
    /// Walk a name-path from the SCENE ROOT. Handles "."/""/ "A/B". None = no such node.
    pub fn resolve_path(&self, path: &str) -> Option<NodeIdx>;
    /// Walk a name-path from an ARBITRARY base node (the script's attaching node).
    /// Honors leading "" / "." as the base itself. M0 does NOT resolve ".."/leading-"/".
    pub fn resolve_path_from(&self, base: NodeIdx, path: &str) -> Option<NodeIdx>;
    /// Unique-name lookup (`%Name`). Slice: scene-wide map. None if no such %-target.
    pub fn resolve_unique(&self, name: &str) -> Option<NodeIdx>;
    /// The node whose body `script = ExtResource(id)` resolves to `script_path` (per-scene half
    /// of Workstream-2 association). Compares against ext_resources[id].path.
    pub fn node_with_script(&self, script_path: &str) -> Option<NodeIdx>;
    /// Child nodes of `idx` (None => the root's children) — for $/get_node completion (M1).
    pub fn children_of(&self, idx: Option<NodeIdx>) -> impl Iterator<Item = (NodeIdx, &SceneNode)>;
}
```

**API notes (decisions):**
- `resolve_path_from` is the **primary** walk M1 uses (a script can attach to a non-root node; `$X` is relative to the attaching node). `resolve_path` is the root convenience wrapper.
- `resolve_unique` keys on the **bare node name** (the `%` is stripped by M1's lexer before calling). M0 builds the map from every node with `unique_name_in_owner == true`.
- These are **pure, allocation-light** lookups (precomputed `by_path`/`unique_nodes`), so they're cheap even uncached in the slice.

---

## 4. The parse strategy (wasm-clean, never-panic)

A **two-pass, line-oriented, byte-offset-tracking** scan. No `std::fs`, no `Instant`, no threads, no regex engine dependency. Operate on `&str` / `&[u8]` with explicit byte indices so every span is a `TextRange`.

### 4.0 Pre-flight: binary detect

Skip leading ASCII whitespace. Read the first up-to-4 non-whitespace bytes:
- `R S R C` (uncompressed) or `R S C C` (compressed) ⇒ **binary** → return `SceneModel { problems: [BinaryResource], .. }` empty. **Stop.**
- `[` ⇒ proceed to text parse.
- anything else (e.g. truncated/garbage) ⇒ best-effort text parse; if pass 1 finds no `gd_scene`/`gd_resource` header, the model is empty (degrade).

### 4.1 Pass 1 — sectionize (the scanner state machine)

Walk the input as a sequence of logical lines, but with a **value-aware** notion of "line" so multiline values don't fool the sectionizer. States:

- **`LineStart`** — at the first non-space char of a fresh line:
  - `;` → **comment**, skip to `\n`. (`#` is NOT handled here — it only appears at value position.)
  - `[` → **header**: scan to the matching `]` *that is outside any quote* (headers are single-line, but be quote-aware so `]` inside `name="a]b"` doesn't close early). Record the header's full byte span. Dispatch on the tag (the first ident). Lex its attributes (§4.2).
  - an identifier (or `/`-containing key) followed eventually by `=` → **body property line**: read the **key**; if key is `script`, `unique_name_in_owner`, or `instance`-class we care about → capture; otherwise enter **value-skip** (§4.3) to consume the (possibly multiline) value losslessly.
  - blank → skip.
- New section begins **only** when a `[` is seen at `LineStart` while **not** inside an open value (depth 0, not in quote).

**Tolerance:** a header that won't lex → `SceneProblem::MalformedHeader`, skip to next `LineStart`-`[`. Never abort the file.

### 4.2 The header value lexer (LOAD-BEARING — C2)

After the tag ident, loop reading `ident WS* = WS* <value>` pairs until `]`. A `<value>` is one of:
1. **quoted string** `"..."` — honor escapes; a `"` ends it; track bytes for spans.
2. **bare ident / int / float / bool / null** — read to the next WS, `]`, or `,`.
3. **`[ ... ]` array** — consume with bracket-depth balance, quote-aware (for `groups`).
4. **`Ident( ... )` constructor** — consume with paren-depth balance, quote-aware (for `instance=ExtResource("id")`, `node_paths=PackedStringArray(...)`). **Extract the inner id** when the ident is `ExtResource`/`SubResource` (quoted-string OR bare-int arg → normalize to the string key).

We **keep** only: `name, type, parent, instance, instance_placeholder, unique_id?(ignored), script_class(on headers), format, uid, load_steps?(ignored), id, path` (per tag). Everything else is consumed and dropped. **Unknown attribute keys are skipped, not errors.**

### 4.3 The body value-skipper (LOAD-BEARING — C12, C11, trailing-comma, CRLF)

To skip a value losslessly we track combined **depth** across `()`, `[]`, `{}`, **quote-aware**, **across physical newlines**:
- Inside a `"..."`: only `\"` and `\\` are special; a bare `\n` is part of the string (C12). `[`/`{`/`(`/`,`/`;`/`#` inside quotes are literal.
- Outside quotes: `#` begins a **Color token** (read the hex run — do **not** treat as comment, C11); `;` to EOL is a comment.
- The value **ends** when depth returns to 0 and we hit end-of-line, **OR** the next line's first non-space char at depth 0 is `[` (new section) or starts a new `key =` at depth 0.
- **Line endings:** normalize `\r\n`/`\r` only for line-counting/`LineStart` detection; **never strip `\r` inside a quoted string** (engine bug #117561 territory — we don't deserialize, so we just pass over it).
- Trailing commas (4.x) and `null` are inert to a depth scanner.

We never *parse* the value's meaning — we only find where it ends. This is why a generic INI crate is wrong and a hand-written skipper is right.

### 4.4 Pass 2 — build the tree

From the collected `[node]` sections (in file order, which is also tree pre-order for siblings):
1. The first node with `parent_path == None` is `root`. Additional parent-less nodes → `SceneProblem::MultipleRoots` (keep the first as root). No parent-less node → `SceneProblem::NoRoot`, `root = None`.
2. For each non-root node, resolve `parent_path` to `parent_idx`:
   - `"."` → root.
   - `"A/B"` → walk `by_name` from root, segment by segment (segments are exact strings incl. spaces). Miss → `SceneProblem::DanglingParent`, leave `parent_idx = None` (node still recorded).
   - Because Godot writes parents before children in file order, a single forward pass with an incrementally-built `(parent_idx, child_name) -> NodeIdx` map resolves all parents without sorting.
3. Compute each node's full name-path (root excluded, matching `parent` semantics) → insert into `by_path`.
4. For each `unique_name_in_owner` node → insert `name -> idx` into `unique_nodes` (slice: scene-wide owner). On a name collision, keep first + record nothing fatal (Godot itself warns; M0 may add a `SceneProblem` later — not required for M0).
5. Validate `script`/`instance` `ExtId`s against `ext_resources`; missing → `SceneProblem::UnknownExtResource` (node keeps `decl_type`/falls to `Node` later).

### 4.5 Never-panic discipline (the invariant)

- All indexing is checked (`get`, not `[]`); all integer parses are `unwrap_or`-defaulted; all `Option`s degrade.
- The function signature is `-> SceneModel` (no `Result`). A catastrophic input yields `SceneModel::empty_with(SceneProblem::…)`.
- A **fuzz/property test** (§8) asserts no panic on arbitrary bytes.

---

## 5. Node-type & script resolution rules — the DATA M0 records (M1 resolves)

M0 records the inputs to typing; it does **not** compute `Ty`. The recorded precedence (for M1's `resolve_node_path`) is:

1. **`decl_type` present** → the node's base class is `type="X"`. **X may be native OR a custom `class_name`** — M1 must look up X in **both** `EngineApi` and the project `class_name` registry, falling back to `Node` (correction to the plan's resolve step 3, which only consulted `EngineApi`).
2. **`decl_type` absent + `instance` present (non-root)** → instanced child sub-scene; base type = referenced `PackedScene`'s root type (M1 recursion / hard tail). M0 records `instance: ExtId`.
3. **`decl_type` absent + `instance` present + root (`instance_is_inherited_root`)** → inherited scene; base type/children come from the base scene (M1, hard tail).
4. **`script` present** → refines (1)/(2): the script's `class_name`/script type is a subtype, strictly more specific. M0 records `script: ExtId` (→ `ext_resources[id].path`/`.uid`).
5. **`script_class=` on the header** → cheap refinement for the **root**/resource without resolving the script file (C8).
6. **Fallback `Node`** at any miss.

**`unique_name_in_owner` (C3):** the body bool, recorded per node and indexed into `unique_nodes`. M1's `%Name` resolves against this map, scoped to the owner (slice: whole scene; tail: per-owner inside instances). **Never** keyed off the header `unique_id` int.

**Parent-path semantics (recorded as `parent_path` + resolved `parent_idx`):** root has none; `"."` = child of root; relative paths exclude the root name; `..`/leading-`/`/absolute `/root/...` are **out of the slice** (need the composed runtime tree) → M1 degrades those to `Node`. M0 does not attempt to resolve `..`/absolute and `resolve_path_from` returns `None` for them (M1 reads that as "degrade to `Node`", which is correct, not an error).

---

## 6. Script↔scene association + the 1-script-many-scenes policy

### 6.1 M0's half (per-scene)

M0 provides `SceneModel::node_with_script(script_path) -> Option<NodeIdx>`: scans nodes whose body `script = ExtResource(id)` resolves (via `ext_resources[id].path`) to `script_path`. The **attaching node** (usually the root, but not always) is what M1 uses as the path-walk base — **not blindly the root** (load-bearing: confirmed by the real `main.tscn` where the script is on the root `[node name="Main" type="Control"]`, and by inherited-scene cases where it's on the root-instance).

### 6.2 The project-wide reverse index (M1, `gdscript-db`) — recorded decision

The `ScriptSceneIndex` (`.gd FileId → Vec<SceneAttachment{scene, node}>`) is **M1**, built by scanning every scene's `ext_resources` (`type="Script"`) — free in Phase 3 since every scene is parsed anyway. **M0 deliberately has no project view.**

### 6.3 The 1-script-many-scenes policy (DECIDED, anchored to Godot)

Godot's own behavior: a `.gd` can attach to many scenes; the editor has no single "the" scene for a script outside an open-scene context. Our settled policy (M1+, recorded here so M0's API shape supports it):

| Policy | Behavior | When |
|---|---|---|
| **Single** | Exactly one owning scene → use it. | Slice default; the overwhelmingly common case. |
| **Common-base union** | N scenes → resolve each path in each scene, take the nearest shared ancestor in the inheritance table (`Button ∪ TextureButton → BaseButton`). **Never wrong, only less specific.** | Full-phase default. |
| **Primary** | A configured/heuristic primary scene (root attaches the script). | Opt-in. |

M0 requirement from this: `SceneModel` and `node_with_script` are **per-scene and composable** — the union/primary logic lives one layer up over a `Vec<SceneModel>`, so M0 needs no awareness of it. ✔ (current shape satisfies this).

---

## 7. Edge cases — each with the chosen M0 behavior

| Edge case | M0 behavior |
|---|---|
| Binary `.scn`/`.res` (RSRC/RSCC) | Detect via magic → empty model + `BinaryResource`. Degrade. |
| `format=2` (3.x) or absent | Parse best-effort (node grammar is stable). Record `format`. Accept bare-int ext ids. |
| `format=4`/unknown | Treat as 4.x family; parse normally (C6). |
| `load_steps` present/absent | Parse-if-present, **ignore value** (C7). Never required. |
| `uid=` absent (header or ext_resource) | `uid = None`. Use `path`. |
| ext_resource missing `type`/`path`/`id` | `MissingExtField`; record what's present; keep going. |
| ext id bare-int (3.x) `id=1`, `ExtResource( 1 )` | Normalize to string key `"1"`; accept surrounding spaces. |
| `type=` is a custom `class_name` | Stored verbatim in `decl_type`; M1 consults class_name registry (corrected resolve rule). |
| Instanced node (no `type=`, `instance=`) | `decl_type=None`, `instance=Some(id)`. |
| Instanced **root** (no `parent=`, `instance=`) | `instance_is_inherited_root=true` (C9). |
| `instance_placeholder="res://..."` | `placeholder=true`, treat like instance for structure. |
| `unique_name_in_owner = true` (body) | `unique_name_in_owner=true`; index by name (C3). |
| header `unique_id=N` | **Ignored** — never confused with the above (C3). |
| `groups=["a","b"]` in header | Lexed as a bracket-array value, dropped (C1). |
| `node_paths=PackedStringArray(...)` in header | Lexed as a constructor value, dropped (C2). |
| Quoted/spaced node names `name="My Button"` | Stored unescaped; per-segment exact match (spaces preserved). |
| Property key with `/` (`surface_material_override/0`) | Recognized as a body key; value skipped. |
| Multiline dict/array/`Packed*Array`/`Object()` body | Depth-balanced lossless skip (§4.3). |
| String with literal `\n` (`text = "a\nb"`) | Skipper stays in-quote across newline (C12). |
| `#RRGGBBAA` in a value | Color token, **not** a comment (C11). |
| `;` comment (line or trailing, outside quotes) | Skipped to EOL. |
| `&"StringName"` / `@"StringName"` | Treated as quoted-string-ish value; skipped. |
| `..` / leading-`/` / `/root/...` parent or path | `parent_idx=None` / `resolve_path_from→None` → M1 degrades to `Node` (not an error). |
| Multiple roots / no root / cyclic parent | `MultipleRoots`/`NoRoot`/`DanglingParent`; pick first root or none; **never loop**. |
| Dangling `script=`/`instance=` id | `UnknownExtResource`; node keeps `decl_type`/falls to `Node`. |
| `[editable path=...]` + interior override `[node parent="Inst/..."]` | Parsed, **adds no new typeable node** (base from sub-scene); recorded structurally only. |
| CRLF / mixed line endings | Normalize for line detection only; never inside quotes. |
| Empty file / whitespace-only / no headers | Empty model, no problems beyond maybe `NoRoot` (only set if a scene with nodes was expected — for empty input, just empty). |

---

## 8. Test plan

### 8.1 Synthetic unit tests (golden `SceneModel` assertions)

- **Header matrix:** `[gd_scene format=3]` (no uid/load_steps); `+uid`; `+load_steps=8`; `+script_class`; `[gd_resource type="Resource" script_class="Dialogue" format=3]`. Assert `format`/`uid`/`script_class`/`kind`.
- **ext_resource matrix:** quoted-string id; bare-int id (3.x); `type="Script"`+uid+path; `type="PackedScene"`; missing-field → `MissingExtField`.
- **node matrix:** root (no parent); `parent="."`; deep `parent="A/B"`; instanced child (`instance=`, no `type=`); inherited root (`instance=`, no parent → `instance_is_inherited_root`); `instance_placeholder`.
- **Body keys:** `script = ExtResource("1")` → `script` set; `unique_name_in_owner = true` → flag + `unique_nodes` entry; a `script` with a **dangling** id → `UnknownExtResource`.
- **Header-value lexer (C2):** node with `unique_id=1975992027 groups=["a","b"] node_paths=PackedStringArray("p") index="0"` → all dropped cleanly, `name`/`type` still correct.
- **Value-skipper (C11/C12/trailing-comma):** multiline `_data = {…}` dict; `color = Color(...)`; `text = "two\nlines"` (literal newline must not start a new property); `bg = #ff8800` (not a comment); `;`-trailing comment; `&"sn"`.
- **Tree:** `by_path["Panel/VBox/StartButton"]`; `resolve_path_from` from a non-root attach node; `resolve_unique`; `node_with_script("res://examples/app.gd")` → the root.
- **Degrade:** binary magic; unknown tag; multiple roots; no root; cyclic parent → no panic, right `SceneProblem`.

### 8.2 Real-corpus robustness gate (0 panics, lossless)

Vendor a fixture set under `crates/gdscript-scene/tests/corpus/` and assert **zero panics + sane tree** (node count, single root, parent links resolve) on **every** file. Required corpus members (named because they exercise the load-bearing cases):
- **`ReactiveUI-Gadot/examples/main.tscn`** (the target project; root-attached script, string-but-non-numeric id `1_app`, no uid, no `load_steps` value-dependence) — the must-pass canonical.
- **godot-demo-projects `2d/dodge_the_creeps/`**: `main.tscn` (instanced `Player`/`HUD`, `script=`+`mob_scene=` on root, multiline `sub_resource` Curve2D, `unique_id=` header ints in master variant, connections), `hud.tscn` (`&"start_game"` StringName, multiline `text`), `mob.tscn` (`groups=["mobs"]`, deeply nested multiline `SpriteFrames` dict/array).
- The engine's own **`modules/gdscript/tests/scripts/completion/get_node/get_node.tscn`** (the authoritative `unique_name_in_owner = true` body fixture + script-on-non-root).
- If reachable: a `Maaack` `main_menu.tscn` (inherited-scene root: `instance=` + body `script=`, no `type=`).
- Optionally vendor **PrestonKnopp/tree-sitter-godot-resource** `test/corpus` as an adversarial value-grammar set (MIT).

> Note (from the dossier): the demo-projects corpus is **not currently on disk** in `vendor/`; M0 must **vendor** the named scenes (or fetch them into `tests/corpus/`) — do not assume they exist. The single guaranteed local scene is `main.tscn`.

### 8.3 Fuzz / property

`cargo fuzz` (native-only target) or a `proptest` harness: arbitrary bytes / mutated real scenes → `parse_scene` **never panics**, always returns a `SceneModel`. Run in CI on native (not part of the wasm gate).

### 8.4 The wasm32 gate (hard CI)

`cargo check -p gdscript-scene --target wasm32-unknown-unknown` green, **every PR**. A tiny wasm smoke test parses a `.tscn` string and resolves one path (no `std::fs`/`Instant`/threads compiled in — enforced by the target itself rejecting them).

### 8.5 Perf (criterion, native)

Parse a ~200-node scene **< 5 ms**; a `resolve_path` (precomputed `by_path`) **< 50 µs**. (Caching is M1; M0's pure parse must already be fast enough to run uncached in the slice.)

---

## 9. Open decisions left for you + plan claims to fix in PHASE-4-SCENE-AWARENESS.md

### 9.1 Genuinely open (need your call)

1. **`ExtId` interning.** M0 uses `ExtId(EcoString)`. M1's salsa layer may want interned ids for cheap keys. **Decision needed:** keep `EcoString` in M0 (simplest, pure) and intern at the `gdscript-db` boundary, or intern in M0 now. *Recommendation: keep `EcoString` in M0 — the pure parser shouldn't depend on an interner; M1 interns.*
2. **Do we record `groups`/`index`/connections now, or drop them?** M0's typing needs none. **Decision:** drop `groups`/`index`; **keep `connection` section spans** only if you want signal-goto in a near milestone (cheap to add now, else defer). *Recommendation: drop all three from the model for M0; the sectionizer already skips them losslessly; add `connections: Vec<Connection>` in a later milestone when signal-goto is scheduled.*
3. **`SceneProblem` surfacing.** Are scene problems ever shown as `.tscn` diagnostics, or purely internal robustness telemetry in M0? *Recommendation: internal-only in M0; M1's `gdscript-ide` decides surfacing.*
4. **Owner scoping for `unique_nodes`.** M0 uses scene-wide (single owner). Confirm we do **not** need per-owner scoping until instanced-sub-scene recursion (M1+ hard tail). *Recommendation: scene-wide in M0 — correct for the slice.*
5. **Should `parse_scene` also accept `project.godot`/`.cfg`?** They share the INI grammar (and Phase 3 injects `project.godot` as text). **Decision:** out of M0 scope — `gd_scene`/`gd_resource` only; a sibling `.cfg` parser (if needed) is a different milestone. *Recommendation: out of scope.*

### 9.2 REFUTED plan claims to fix in `PHASE-4-SCENE-AWARENESS.md`

Apply these edits when you revise the design doc (each is a load-bearing correctness fix, not a style nit):

- **§1.1 example (line 155):** move `unique_name_in_owner=true` **out of the `[node ...]` header** and onto its own body line `unique_name_in_owner = true`; it is never a header attribute (C3). Add `unique_id` as the (ignored) header int and explicitly state the two are unrelated.
- **§1.4 edge table:** change `groups=PackedStringArray(...)` → **`groups=["a","b"]` bracket array**; note `node_paths=PackedStringArray(...)` is the constructor form (C1). Add the rule **"header values are not all quoted — accept bare ints/bools/`[...]`/`Type(args)`"** (C2). Remove `^"..."` from the `.tscn` value list (it's source syntax; move to Workstream 3, C10). Add **"`#` is a Color literal, not a comment; `;` is the only comment char"** (C11). Add **"quoted strings can contain literal newlines"** (C12).
- **§1.1 section table:** expand the recognized tags from 5 to **8** (add `gd_resource`, `resource`, `editable`) and add `script_class=` to the scene/resource header row (C5, C8).
- **§1.1 / §1.4 format note:** `load_steps` is **deprecated/optional** (omit-dependence), and `format>=3` is the 4.x family (treat `format=4` as 4.x, don't branch on literal `3`) (C6, C7).
- **"Why this matters" (line 108):** soften "architecturally cannot" to **"completion-only, editor-bound, and does not flow a persistent `Ty` into inference/hover/diagnostics"** — the engine *does* scene-aware completion in `TOOLS_ENABLED`; our edge is standalone headless **type-flow** (C4).
- **Workstream 3 resolve step 3 (line 364):** add that `type="X"` must be resolved against the **class_name registry** as well as `EngineApi` (custom `class_name` nodes), not native-only.
- **§1.2 model:** add `script_class`, `resource_type`, `instance_is_inherited_root`, `instance_placeholder`, and `sub_resources` as `id→SubResource{type,span}` (was `id→type`); add `name_span` to `SceneNode`.

---

### Files referenced
- Plan to amend: `C:/Yanivs/GameDev/gdscript-analyzer/plans/PHASE-4-SCENE-AWARENESS.md`
- House-style reference: `C:/Yanivs/GameDev/gdscript-analyzer/plans/PHASE-3-IMPLEMENTATION-PLAYBOOK.md`
- Canonical must-pass corpus scene (local): `C:/Yanivs/GameDev/ReactiveUI/ReactiveUI-Gadot/examples/main.tscn`
- Crate to create: `C:/Yanivs/GameDev/gdscript-analyzer/crates/gdscript-scene/` (`src/model.rs`, `src/parse.rs`, `tests/corpus/`)

This is the document M0 is built from. The five hardest correctness facts to get right (verify these first against the corpus): **C1** (groups = bracket array), **C2** (unquoted header values), **C3** (`unique_name_in_owner` body bool vs `unique_id` header int), **C11** (`#` = color, not comment), **C12** (literal newlines inside quoted strings).