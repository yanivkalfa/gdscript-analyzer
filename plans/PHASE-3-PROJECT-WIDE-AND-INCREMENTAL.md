# PHASE 3 — Project-Wide & Incremental (Tier 2)

> **Status:** plan. **Tier:** 2 (project-wide graph + incremental). **This is the highest-engineering-risk phase** ([`ROADMAP.md`](ROADMAP.md): *"the single biggest risk is project-wide incremental invalidation"*).
> **Canonical parents this doc obeys:** [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§1 crate stack, §2 the `AnalysisHost`/`Analysis` API, §3 the salsa-at-Tier-2 decision, §7 portability), [`ROADMAP.md`](ROADMAP.md) (Phase 3 deliverable + exit criteria, the Tier-2 placement, "biggest risk = invalidation").
> **Primary evidence:** [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) (salsa 0.27, `AnalysisHost`/snapshot, cancellation, durability, the MVP→v1 migration), [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) (the project graph, `class_name` registry, `DefMap`, the incremental invariant), [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) (`project.godot`, autoloads + the `*` flag, `.godot/global_script_class_cache.cfg`, `res://` resolution, the five linking mechanisms).

This phase turns *"we understand one `.gd` file"* (Phase 2, Tier 1) into *"we understand the whole Godot project."* It is where the analyzer grows a **project model** (`gdscript-db`), a **global `class_name` registry**, **cross-file resolution** of `extends`/`preload`/`load`, and the **navigation features** that need a project-wide index (go-to-definition, find-references, rename, workspace symbols). It is also where **salsa is adopted** so all of that stays **keystroke-fast** as the project grows.

Everything here was *designed for* in Phase 2: the single `resolve_external(...) -> Ty::Unknown` seam, the distinct `Ty::Unknown`/`Ty::ScriptRef` variants, and the pure `(db, file) -> value` query functions. **Phase 3 lights up that seam and swaps the cache engine — it does not rewrite the checker.**

---

## Goal & scope (Tier 2; single-file → project; salsa adoption)

### What ships

1. **The project model in `gdscript-db`** — workspace discovery (find `project.godot` = the `res://` root), a VFS of all `.gd`/`.tscn`/`.tres`, a `project.godot` parser → `[autoload]` singletons (the `*` global flag), `[global_group]`, `[input]` actions, and **Godot version detection** from `[application] config/features`. (The `SourceRoot`/crate-graph analog.)
2. **The global `class_name` registry** — scan every `.gd` for `class_name` declarations → a project-wide `name → (FileId, DefId)` map; the `.godot/global_script_class_cache.cfg` is read as a **warm-start hint only**, re-derived from source and treated as authoritative *only* from the `.gd` files. Autoload singleton names are globals too.
3. **Cross-file resolution** — `extends "res://x.gd"` / `extends GlobalClass`, `const X = preload("res://x.gd")`, `load("res://…")` literal, and bare `class_name` references resolve to a `FileId` + its exported script type; the **cross-file inheritance chain** (`script extends script extends Control…`) is built; member lookup now spans the **script graph + the engine API model**. This **fills Phase 2's `resolve_external → Unknown`**.
4. **Cross-file IDE features (`gdscript-ide`)** — **go-to-definition** across files, project-wide **find-references**, **rename** (a `SourceChange` spanning files), and **workspace symbols** (querying the global registry).
5. **salsa adopted (the incremental core)** — `gdscript-db` internals become a salsa 0.27 query graph: `#[salsa::input]` for files/config, `#[salsa::tracked]` for derived queries, `#[salsa::interned]` for symbol ids, `#[salsa::accumulator]` for diagnostics; red-green invalidation with **durability** tiers; cancellation wired to `Cancellable<T>` at the `ide` boundary. The Phase-2 pure fns become tracked queries — **a localized change.**

### The invariant that defines this phase (the headline)

> **Editing a function body never invalidates project-global data.**

This is the load-bearing property ([`research/09`](research/09-type-system-and-inference.md) §3.1, [`research/06`](research/06-analyzer-architecture.md) §1). The `ItemTree` deliberately **excludes function bodies**, so a body edit changes only `body(file, fn)` → `infer(file, fn)` → `diagnostics(file)` and touches **nothing** in the `class_name` registry, the project graph, or any other file. Combined with salsa **durability** (the Godot stdlib API is `HIGH` durability; the edited file is `LOW`), this is what keeps a *project-wide* analyzer as fast per keystroke as the single-file one — independent of project size. **Preserving this invariant is the success condition for Phase 3.**

### Explicit non-goals (deferred)

| Deferred capability | Phase | Why not here |
|---|---|---|
| Scene-aware node-path typing (`$Path`/`%Unique`/`get_node("…")` → concrete `Button`) | **4** | Needs `.tscn`/`.tres` parsing (`gdscript-scene`). Phase 3 still types these to `Node`; it adds the *file graph* scenes will hang off. The VFS already ingests `.tscn`/`.tres` so Phase 4 is purely additive. |
| Recursive instanced sub-scene typing, `@onready` timing | **4/6** | Built on scene awareness. |
| The full **48-warning** set + project-settings gating + warnings-as-errors | **6** | Phase 3 *parses* `[debug] gdscript/warnings/*` into the project model and adds the **gating filter layer** (severity per code, `@warning_ignore`), but the full warning *set* is Phase 6. |
| Real control-flow-graph narrowing that beats Godot | **6** | Orthogonal to project-wide; Phase 3 keeps Phase 2's local syntactic narrowing. |
| Call hierarchy, project-wide rename of files/UIDs, structural search-replace | **6** | Need the full graph + scene model; out of the Tier-2 navigation core. |
| Multi-threaded read parallelism beyond single-writer/multi-reader | **5/6** | MVP threading = single-writer/multi-reader (fork snapshots). Native worker-pool parallelism is a later, localized turn-on (all query types are already `Send`). |

**Boundary rule (preserved from Phase 2, now satisfied):** the checker still funnels every cross-file question through `resolve_external(...)`. Phase 3 reimplements **only** that function (now backed by salsa queries over the project graph). The checker body is unchanged; `Ty::Unknown` mostly disappears (an external ref now resolves to a real `Ty::ScriptRef`/`Ty::Object`), surviving only for genuinely unresolvable refs (dynamic `load(var)`, a path to a missing file).

---

## Prerequisites

**From Phase 2 (Tier 1 — single-file inference), the seam this phase fills:**
- `gdscript-hir`: the binder + the forward checker, with **`resolve_external(what: ExternalRef) -> Ty`** as the *single* cross-file seam (Phase 2 returns `Ty::Unknown`). The `Ty` enum already has the distinct **`ScriptRef(ScriptRefId)`** (a script-class type referenced by name/path) and **`Unknown`** (deferred-to-Phase-3 marker) variants — Phase 3 makes `resolve_external` return real types and lights up `ScriptRef`.
- `gdscript-hir`: the `ItemTree` (signatures, **body-excluding** — already the right granularity for the invariant) and per-function `Body`, both produced by **pure `(db, file) -> value` fns** (`item_tree(db, file)`, `body(db, file, fn)`, `infer(db, file, body)`). These are exactly the fns that become `#[salsa::tracked]`.
- `gdscript-hir`: `ExtendsRef` already models `Native(ClassId) | ScriptPath(String) | Implicit(RefCounted)`; Phase 2 resolves only `Native`. Phase 3 resolves `ScriptPath` (and bare-name `class_name` extends).
- `gdscript-api`: the read-only `EngineApi` (inheritance table, `lookup_member`, `is_subclass`) — the **native half** the script graph merges with. Unchanged; loaded at `HIGH` durability in Phase 3.
- `gdscript-ide`: the `AnalysisHost`/`Analysis` pair with `apply_change(Change)` and `Cancellable<T>` already on every query (Phase 1/2 returned `Cancellable` even though nothing cancelled yet). Phase 3 makes cancellation *real*.
- `gdscript-base`: `FileId`, `TextRange`, `LineIndex`, the POD result structs, and the `SourceChange`/`CodeAction` POD (Phase 2 code actions already produce a `SourceChange` — rename reuses it project-wide).

**From Phase 0 (project conventions & tooling):**
- The `res://` ↔ filesystem convention: `res://` = the directory containing `project.godot` ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.2). The VFS path scheme and the `Change` type (file add/edit/remove + config) exist in `gdscript-base`.
- Multi-version `gdscript-api` bundling + selection logic (Phase 0 / [`GODOT-SYNC.md`](GODOT-SYNC.md)); Phase 3 wires *project-version detection* (from `project.godot`) into the selection that Phase 2 hard-wired to the default minor.
- `xtask` fixtures: real Godot projects vendored as multi-file test fixtures (the corpus this phase validates against).

**Sanity gate before starting:** the Phase-2 tree is green, including `cargo check -p gdscript-ide --target wasm32-unknown-unknown` (the portability invariant — **salsa must stay wasm-safe**, see Workstream 4 + Risks).

---

## Workstream 1 — The project model & workspace input (`gdscript-db`)

`gdscript-db` is rust-analyzer's `base-db` analog ([`research/06`](research/06-analyzer-architecture.md) §1): it owns the **inputs** (file text, project config, the engine API version) and knows **nothing** about the filesystem — files are injected through the VFS as opaque `FileId`s ([`01`](01-ARCHITECTURE.md) §7: *no `std::fs` in the core*). This phase grows it from "a map of open files" into a real **project model** + (Workstream 4) a salsa input layer.

### 1.1 Workspace discovery & the VFS

- **The root = `project.godot`.** Workspace discovery finds the `project.godot` nearest to / above the opened files; its directory is the **`res://` root** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.2). All `res://…` paths resolve against it. A workspace with no `project.godot` degrades to single-file mode (Phase 2 behavior) — never an error.
- **The VFS** holds every `.gd`, `.tscn`, and `.tres` under the root, plus `project.godot` and (as a hint) `.godot/global_script_class_cache.cfg`. Each is a `FileId → text` input. `.tscn`/`.tres` are ingested **now** (so Phase 4 is additive) but only *parsed* in Phase 4.
- **File watching is a CLIENT concern.** The host does **not** watch the disk ([`research/06`](research/06-analyzer-architecture.md) §7, [`01`](01-ARCHITECTURE.md) §7). The LSP/CLI/playground client watches files (or receives editor `didChange`/`didOpen`) and pushes a `Change` through `apply_change`. The library is filesystem-agnostic; the *client* maps paths → `FileId` and reads bytes. This keeps the core wasm-safe and the threading model a single writer.
- **`res://` ↔ `FileId` mapping** lives in `gdscript-db` as a pure table (`res_path: EcoString → FileId`), populated from the VFS keyed by each file's project-relative path. `load(var)` (non-literal) never hits it → `Variant`.

### 1.2 Parsing `project.godot` (INI-like, typed-Variant values)

`project.godot` is a `ConfigFile`/INI: `[section]` headers, `key=value` where **values are typed Variants** (`VariantWriter` encoding — quoted strings, ints, `Vector3(...)`, `Object(InputEventKey,...)`), `;` comments, slash keys split into section+sub-key ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.6). `config_version=5` ⇒ Godot 4.x. We parse only the sections we need; we do **not** evaluate arbitrary Variant `Object(...)` payloads (we keep `[input]` event lists opaque — only action names matter for completion).

```rust
// crates/gdscript-db/src/project.rs  (sketch — illustrative, not final)

/// The parsed project.godot. A salsa-tracked value (Workstream 4) derived from the
/// project.godot FileId. HIGH-ish durability: rarely edited relative to source files.
pub struct ProjectModel {
    pub root: FileId,                          // the project.godot FileId (res:// anchor)
    pub godot_version: GodotVersion,           // from [application] config/features (§1.3)
    pub main_scene: Option<EcoString>,         // [application] run/main_scene (res:// path)
    pub autoloads: Vec<Autoload>,              // [autoload] (§Workstream 2)
    pub global_groups: Vec<EcoString>,         // [global_group] names (when present)
    pub input_actions: Vec<EcoString>,         // [input] action names (for completion; events opaque)
    pub warning_config: WarningConfig,         // [debug] gdscript/warnings/* (the gating layer; §5 notes)
}

pub struct Autoload {
    pub name: EcoString,        // the [autoload] key, e.g. "PlayerVariables"
    pub path: EcoString,        // res:// path to a .gd or .tscn
    pub is_singleton: bool,     // the leading '*' => a GLOBAL identifier (research/04 §3.3)
}

/// Per-code severity overrides + master toggles. The CHECKER always emits; this gates.
pub struct WarningConfig {
    pub enable: bool,                                  // debug/gdscript/warnings/enable
    pub treat_as_errors: bool,                         // .../treat_warnings_as_errors
    pub per_code: FxHashMap<EcoString, WarnLevel>,     // lowercase code -> Ignore|Warn|Error
    pub exclude_addons: bool,                          // 4.3/4.4; res://addons/* suppressed
    // master `directory_rules` (4.x master) parsed but applied in Phase 6.
}
pub enum WarnLevel { Ignore, Warn, Error }
```

- **`[autoload] Name=*res://path`** — the leading `*` sets `is_singleton = true`, meaning `Name` is a **global GDScript identifier** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.3). Strip the `*` to get the path. No `*` ⇒ a node reachable only via `get_node("/root/Name")`, **not** a global — we record it but do not seed the global table.
- **`[global_group]`** — present only when global groups exist; names feed `add_to_group`/group-name completion (a small win; not load-bearing).
- **`[debug] gdscript/warnings/*`** — parsed into `WarningConfig`. Phase 3 adds the **gating filter** (map `code → severity`, suppress `Ignore`, escalate on `treat_as_errors`, honor inline `@warning_ignore("name")`) as a *post-checker* layer keyed on the stable `DiagnosticCode` — the checker is untouched (Phase 2 §5.3 designed for exactly this). The full 48-warning *set* is Phase 6.

### 1.3 Godot version detection

From `[application] config/features` (a `PackedStringArray` like `["4.4", "Forward Plus"]`) — the first version-shaped entry is the project's Godot minor ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.6, [`01`](01-ARCHITECTURE.md) §5). Snap to the **nearest bundled `gdscript-api` minor** (newest as default, override allowed), exactly the Phase-0 selection logic — Phase 3 just feeds it the *detected* version instead of the hard-wired default. `config_version=5` is a coarse 4.x discriminator if `features` is absent. The selected `ApiVersion` becomes a **salsa input** at `HIGH` durability (it changes ~never during a session).

### 1.4 The `SourceRoot` / crate-graph analog

rust-analyzer separates `SourceRoot`s (a directory of files) from the crate graph ([`research/06`](research/06-analyzer-architecture.md) §1). Our analog is simpler: **one project = one `res://` root = one resolution domain.** The "graph" is the implicit DAG of `extends`/`preload`/`load`/autoload edges over the files in that root (Workstream 3). `res://addons/*` is a sub-root for gating (`exclude_addons`) but resolves into the same domain. Multi-project workspaces (rare) = multiple roots, each its own `ProjectModel`; cross-root references are not a Godot concept and are out of scope.

---

## Workstream 2 — The global symbol registry (`class_name`)

The first project-wide registry feeding global scope ([`research/09`](research/09-type-system-and-inference.md) §2.3, §4.2). It turns a bare `Item` (no `preload`) into a usable type anywhere in the project.

### 2.1 Scan source, treat the cache as a hint

> **Scan `.gd` directly; the `.godot/global_script_class_cache.cfg` is a warm-start hint only — never trusted as truth.**

The cache is frequently **stale or fails to regenerate**, isn't present until the project is opened in the editor, and silently drops pre-4.4 entries on upgrade ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.1, §3.7; [`research/09`](research/09-type-system-and-inference.md) §4.2). And we **already parse every `.gd`** to build its `ItemTree`, so extracting `class_name`/`extends`/`@icon`/`@tool`/`@abstract` during that pass is nearly free and **always correct**. Use the cache **only** to prioritize which files to index first on cold start (a latency optimization), then reconcile against the source-derived truth and discard divergences.

### 2.2 The registry type

```rust
// crates/gdscript-db/src/global_registry.rs  (sketch)

/// Interned id for a project-global definition (a class_name class, an autoload singleton).
/// #[salsa::interned] in Workstream 4 — cheap Copy equality, stable across revisions.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct DefId(u32);

/// The project-wide name -> definition map. A salsa-tracked value derived from the set of
/// ItemTrees (class_name half) + ProjectModel (autoload half). Rebuilt ONLY when a
/// class_name / extends / autoload changes — NOT on a body edit (the invariant).
pub struct GlobalRegistry {
    by_name:   FxHashMap<EcoString, GlobalDef>,   // "PlayerController" -> def; "Utils" -> def
    /// Diagnostics for collisions (two files claim the same class_name; a class_name that
    /// shadows an autoload or an engine class). Surfaced via the accumulator.
    collisions: Vec<GlobalCollision>,
}

pub enum GlobalDef {
    /// `class_name Foo` in some file. The file IS the class.
    ClassName { def: DefId, file: FileId, decl_range: TextRange,
                base: ExtendsRef, icon: Option<EcoString>, is_tool: bool, is_abstract: bool },
    /// An [autoload] *singleton. Its type is the script's class (or scene-root type in Phase 4).
    Autoload  { def: DefId, name: EcoString, target: FileId, decl_in_project_godot: TextRange },
}

impl GlobalRegistry {
    pub fn resolve(&self, name: &str) -> Option<&GlobalDef> { self.by_name.get(name) }
    pub fn iter(&self) -> impl Iterator<Item = (&EcoString, &GlobalDef)> { self.by_name.iter() } // workspace symbols
}
```

### 2.3 Collisions, shadowing, and precedence

- **Two files claim the same `class_name`** → a diagnostic on both (Godot itself errors: *"Class 'X' hides a global script class"*); we keep the **first by deterministic file order** as the resolvable one and flag the rest, so resolution never panics and is stable across runs.
- **A `class_name` colliding with an autoload** → Godot errors (*"hides an autoload singleton"* — [`research/04`](research/04-gdscript-semantics-and-features.md) §3.3); we record both and prefer the autoload for the global identifier (matching the engine), diagnosing the `class_name`.
- **A `class_name` shadowing an engine class** (e.g. `class_name Node`) → diagnostic; engine name wins for type positions.
- **Precedence in global scope** (extends Phase 2's §3.2 step 4): `@GlobalScope` builtins/utilities/global enums/consts → builtin type names → **`class_name` globals** → **autoload singleton names** → (engine singletons `Input`/`OS` already in the API model). Locals/members/inherited still win first (Phase 2's local → member → inherited → global order is unchanged; this only fills the *global* tier).

### 2.4 Autoload globals are names too

Each `is_singleton` autoload injects a global identifier whose type is its target script's class ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.3, [`research/09`](research/09-type-system-and-inference.md) §2.3). `PlayerVariables.health -= 10` resolves `PlayerVariables` → the autoload's `FileId` → its `ItemTree`'s class type → member `health` via the cross-file member lookup (Workstream 3). A `.tscn` autoload types to its scene-root class (the type sharpens in Phase 4; Phase 3 types it to the root node's declared type or `Node`).

---

## Workstream 3 — Cross-file resolution (the `DefMap` goes project-wide)

This is where `resolve_external` stops returning `Unknown`. The five linking mechanisms ([`research/04`](research/04-gdscript-semantics-and-features.md) §3, [`research/09`](research/09-type-system-and-inference.md) §4.4): `class_name` globals, `preload`/`load`/`extends "path"`, autoloads, (scenes → Phase 4), and project settings. Phase 3 implements the four script-level ones into one **`Ty` lattice** spanning the script graph + the engine API ([`research/09`](research/09-type-system-and-inference.md) §3.3, §6).

### 3.1 Resolving `extends`

`extends` has three forms ([`research/04`](research/04-gdscript-semantics-and-features.md) §1.5, [`research/09`](research/09-type-system-and-inference.md) §1.3): `extends NativeOrGlobalClass`, `extends "res://x.gd"`, `extends "res://x.gd".InnerClass`. Resolution (extending Phase 2's `ExtendsRef`):

- `extends Button` → native: `EngineApi::class_by_name` (Phase 2 already did this).
- `extends MyGlobalClass` → `GlobalRegistry::resolve` → another file's `ItemTree` (the script base).
- `extends "res://x.gd"` → `res://` map → `FileId` → its `ItemTree` (the script base).
- `extends "res://x.gd".Inner` → that file's inner-class `ItemTree`.
- Missing `extends` → `RefCounted` (Phase 2).

A **script base** means the inheritance chain now mixes script and native links: `PlayerController.gd extends "res://char.gd" extends CharacterBody2D (native) extends PhysicsBody2D → … → Object`. Member lookup walks **script `ItemTree`s first, then crosses into the native `EngineApi` table** at the first native base — one unified walk.

### 3.2 Resolving `preload` / `load`

- `const X = preload("res://x.gd")` — the **key static hook** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.2). `preload(constString)` → `res://` map → `FileId` → a `Ty::ScriptRef` for that file's class. Then `X.new()` → an instance of the class, `var v: X`, `v is X`, and `X.Inner` / `X.SOME_CONST` all resolve.
- `load("res://x.gd")` with a **string literal** → resolved identically to `preload` (Godot treats it the same statically).
- `load(var)` (dynamic, non-literal) → `Ty::Variant` (genuinely unknowable). This is the *only* common case left producing `Variant` here.
- `preload("res://scene.tscn")` → `PackedScene` (Phase 3); the scene-root type is Phase 4.

### 3.3 The cross-file resolver entry (what `resolve_external` becomes)

```rust
// crates/gdscript-hir/src/resolve_cross_file.rs  (sketch — THE Phase-3 seam, now backed by the graph)

/// Phase 2 returned Ty::Unknown for ALL of these. Phase 3 resolves them through the project graph.
/// Still the ONE function the checker calls for any cross-file question — nothing else changed.
pub fn resolve_external(db: &dyn Db, from: FileId, what: ExternalRef<'_>) -> Ty {
    match what {
        // bare `Foo` not native/builtin/in-file -> class_name registry (or autoload name)
        ExternalRef::ClassName(name) => match global_registry(db).resolve(name) {
            Some(GlobalDef::ClassName { def, .. }) => Ty::ScriptRef(script_ref_of(db, *def)),
            Some(GlobalDef::Autoload  { target, .. }) => script_ty_of_file(db, *target),
            None => Ty::Unknown,                       // truly undefined -> Unknown (diagnosable)
        },
        // extends "res://x.gd"  /  extends Global
        ExternalRef::ExtendsPath(res_path) => match res_to_file(db, res_path) {
            Some(file) => script_ty_of_file(db, file),
            None => Ty::Error,                          // dangling base -> error (a real bug in user code)
        },
        // const X = preload("res://x.gd")  /  load("res://x.gd") literal
        ExternalRef::Preload(res_path) | ExternalRef::LoadLiteral(res_path) =>
            match res_to_file(db, res_path) {
                Some(file) => Ty::ScriptRef(script_ref_of_file(db, file)),
                None => Ty::Error,                      // missing preload target -> error
            },
        // an [autoload] *singleton identifier used as a value
        ExternalRef::Autoload(name) => match global_registry(db).resolve(name) {
            Some(GlobalDef::Autoload { target, .. }) => script_ty_of_file(db, *target),
            _ => Ty::Unknown,
        },
        ExternalRef::LoadDynamic => Ty::Variant,        // load(var) — genuinely unknowable
    }
}
```

`script_ty_of_file` / `script_ref_of_file` build (or fetch the cached) `Ty::ScriptRef` whose member set is that file's `ItemTree` flattened over its `extends` chain (§3.4). All of these are **salsa queries** (Workstream 4), so `resolve_external` is memoized and incrementally invalidated for free.

### 3.4 Cross-file member lookup (the unified walk)

Phase 2's `EngineApi::lookup_member` walked the native chain. Phase 3 generalizes to a **two-realm walk** over the merged graph ([`research/09`](research/09-type-system-and-inference.md) §3.3, §6):

```rust
/// Walk script ItemTrees up the `extends` chain; cross into EngineApi at the first native base.
/// Returns the first declarer of `name`. Used by infer_field / infer_method_call for ScriptRef receivers.
pub fn lookup_member_cross_file(db: &dyn Db, ty: ScriptRefId, name: &str) -> Option<MemberRef> {
    let mut cursor = ScriptChainCursor::start(db, ty);
    while let Some(link) = cursor.next(db) {           // each link: a script ItemTree OR a native ClassId
        match link {
            Chain::Script(file) => if let Some(m) = item_tree(db, file).member(name) { return Some(m); },
            Chain::Native(class) => return engine_api(db).lookup_member(class, name), // rest is native
        }
    }
    None
}
```

- **Cycle detection:** cyclic `extends` is illegal in Godot ([`research/09`](research/09-type-system-and-inference.md) §4.4); the cursor caps the walk and emits a `CYCLIC_INHERITANCE` diagnostic instead of looping forever. (preload cycles are *legal* at runtime and must **not** be reported — only `extends` cycles are errors.)
- **`super` / `super.method()`** now resolves against a script base (Phase 2 returned `Unknown`) — walks from the base link.
- **Variant absorption unchanged:** member miss on a *known* script type → `UNSAFE_PROPERTY_ACCESS`/`UNSAFE_METHOD_ACCESS` (same as native), because a subtype might have it. `Unknown`/`Variant` receivers behave exactly as Phase 2.

The net effect: every `Ty::Unknown` Phase 2 produced for a resolvable external now becomes a real type, and **member completion / hover / diagnostics light up across files with zero checker changes.**

---

## Workstream 4 — salsa adoption (the incremental core)

The reason this phase exists as a distinct tier. Phase 2 used plain maps + whole-file recompute ([`01`](01-ARCHITECTURE.md) §3, [`research/06`](research/06-analyzer-architecture.md) §8). Project-wide resolution makes "recompute everything per keystroke" untenable, so we adopt **salsa 0.27** (macro-based "new salsa" — [`research/06`](research/06-analyzer-architecture.md) §2). The public `ide` API is **unchanged**; only the engine behind `gdscript-db` changes.

### 4.1 The query graph

```
INPUTS (#[salsa::input] — set via apply_change)            DURABILITY
  SourceFile { res_path, #[returns(ref)] text }            LOW   (edited files)
  ProjectConfig { #[returns(ref)] project_godot_text }     MEDIUM(project.godot)
  ApiInput { version, #[returns(ref)] api_bytes }          HIGH  (Godot stdlib API)
        │
        ▼  #[salsa::tracked] fns (pure, memoized, body-excluding where it matters)
  parse(file)            -> Parse           (CST + AST)            ── per file
  item_tree(file)        -> Arc<ItemTree>   (SIGNATURES; NO bodies)── per file   ★ the invariant lives here
  project_model(cfg)     -> Arc<ProjectModel>                      ── from project.godot
  engine_api(api)        -> Arc<EngineApi>                         ── HIGH durability, ~never recomputed
  global_registry(db)    -> Arc<GlobalRegistry> (class_name + autoload) ── from {item_tree(*)} + project_model
  def_map(file)          -> Arc<DefMap>     (scopes + cross-file resolutions for this file)
  body(file, fn)         -> Arc<Body>                              ── per function   ★ body edits stop here
  infer(file, fn)        -> Arc<InferenceResult>                   ── per function   ★ and here
  diagnostics(file)      -> Vec<Diagnostic>  (via accumulator)     ── per file
        │
        ▼  #[salsa::interned]        DefId, ScriptRefId, Name  (cheap Copy equality across revisions)
        ▼  #[salsa::accumulator]     Diagnostics  (the side-channel; pushed during tracked fns)
```

```rust
// crates/gdscript-db/src/db.rs  (sketch — the salsa declarations)

#[salsa::input]
pub struct SourceFile {
    pub res_path: EcoString,
    #[returns(ref)] pub text: String,
}
#[salsa::input]
pub struct ProjectConfig { #[returns(ref)] pub project_godot_text: String }
#[salsa::input]
pub struct ApiInput { pub version: ApiVersion, #[returns(ref)] pub api_bytes: Vec<u8> }

#[salsa::interned] pub struct DefId<'db>       { pub raw: u32 }
#[salsa::interned] pub struct ScriptRefId<'db> { pub file: FileId, pub inner: Option<u32> } // inner class
#[salsa::interned] pub struct Name<'db>        { #[returns(ref)] pub text: String }

#[salsa::accumulator]
pub struct Diagnostics(pub Diagnostic);        // Diagnostics::push(db, d) in tracked fns

#[salsa::tracked] pub fn item_tree(db: &dyn Db, file: SourceFile) -> Arc<ItemTree> { /* signatures only */ }
#[salsa::tracked] pub fn global_registry(db: &dyn Db) -> Arc<GlobalRegistry> { /* scan all item_trees */ }
#[salsa::tracked] pub fn infer(db: &dyn Db, file: SourceFile, func: BodyId) -> Arc<InferenceResult> { /* … */ }

#[salsa::db]
#[derive(Default, Clone)]
pub struct RootDatabase { storage: salsa::Storage<Self> }
#[salsa::db] impl salsa::Database for RootDatabase {}
```

### 4.2 Which salsa primitive, and the durability tiers

| Item | salsa primitive | Durability | Notes |
|---|---|---|---|
| `SourceFile.text` (a `.gd`) | `#[salsa::input]` | **LOW** (volatile) | the file being typed; bumps revision every keystroke |
| `ProjectConfig` (`project.godot`) | `#[salsa::input]` | **MEDIUM** | edited rarely vs source; an edit may invalidate the registry/autoloads |
| `ApiInput` (Godot stdlib API + version) | `#[salsa::input]` | **HIGH** (durable) | unchanged for a whole session; lets queries reading *only* the API skip re-validation entirely |
| `parse` / `item_tree` / `body` / `infer` / `def_map` / `diagnostics` | `#[salsa::tracked]` fn | — | pure, memoized; red-green re-validates only what actually changed |
| `DefId` / `ScriptRefId` / `Name` | `#[salsa::interned]` | — | symbol ids; Copy equality, stable identity for matching prev-revision values |
| `Diagnostics` | `#[salsa::accumulator]` | — | the diagnostic side-channel: `infer`/`item_tree` push; `diagnostics(file)` reads `accumulated::<Diagnostics>` |

**Durability** ([`research/06`](research/06-analyzer-architecture.md) §2, the durable-incrementality post): salsa keeps a per-durability revision vector. When a **LOW** input (the edited file) changes, any query that read **only HIGH** inputs (e.g. a hover that touched just the engine API) **skips the entire subgraph** and never re-validates. This is *the* mechanism that keeps keystroke latency flat as the project grows — the bulk of the project (other files' signatures, the stdlib API) is higher-durability than the one file under the cursor.

### 4.3 The invalidation invariant + how ItemTree-excludes-bodies enforces it

> **Editing a function body invalidates `body(file, fn)` → `infer(file, fn)` → `diagnostics(file)` and NOTHING else.**

This holds **because `item_tree(file)` excludes function bodies** ([`research/09`](research/09-type-system-and-inference.md) §3.1, [`research/06`](research/06-analyzer-architecture.md) §1). A body edit changes the file's `text` (LOW), which forces `parse(file)` to re-run — but `item_tree(file)` re-derives to a value **equal** to the previous revision (signatures didn't move), so salsa's red-green algorithm **stops the wave there**: `global_registry`, every other file's `def_map`, the project graph — none re-run. Only the edited function's `body`/`infer` recompute. Contrast a **signature** edit (rename a `func`, change a param type, add/remove a `class_name`): `item_tree(file)` *does* change, so dependents re-validate — but **bounded by the dependency graph**, not the whole project ([`research/09`](research/09-type-system-and-inference.md) §4.5).

**This invariant is enforced by construction (the ItemTree/Body split) and verified by test** (Workstream 6 #4: a body edit produces an empty global-data recomputation set in salsa's event log). If a future change makes `item_tree` depend on a body, the invariant breaks silently — hence the test is a **CI gate**, not a nicety.

### 4.4 Cancellation wiring to `Cancellable<T>`

salsa gives cancellation for free ([`research/06`](research/06-analyzer-architecture.md) §7, [`01`](01-ARCHITECTURE.md) §2): a concurrent `apply_change` (which takes `&mut db`) **cancels in-flight reads** on snapshots via a special panic that unwinds to the query boundary. The `ide` layer is **the boundary that catches it** and turns it into `Cancellable<T>`:

```rust
// crates/gdscript-ide  — the catch boundary (Phase 1/2 returned Cancellable but never cancelled)
impl Analysis {
    pub fn hover(&self, pos: FilePosition) -> Cancellable<Option<HoverResult>> {
        salsa::Cancelled::catch(|| self.hover_impl(pos))   // salsa panic -> Err(Cancelled); client re-issues
    }
}
```

- **Threading (MVP): single-writer / multi-reader** ([`research/06`](research/06-analyzer-architecture.md) §7). The `AnalysisHost` owner (the LSP main loop) applies changes on one thread and **forks `Analysis` snapshots** for reads. Long queries call `unwind_if_cancelled` to poll. A newer keystroke cancels the now-stale read; the client re-issues against the fresh snapshot.
- **wasm:** single-threaded, synchronous — `Cancelled::catch` still works (cancellation just never fires mid-query because there's no concurrent writer); same code path, no thread APIs compiled in (feature-gated — see Risks).

### 4.5 Migration: convert the Phase-2 pure fns to tracked queries

> A **localized change, not a rewrite** — the architecture was designed for it ([`01`](01-ARCHITECTURE.md) §3, [`research/06`](research/06-analyzer-architecture.md) §8).

Each Phase-2 pure `(db, file) -> value` fn becomes a `#[salsa::tracked]` fn by:
1. Replacing the hand-rolled `HashMap<FileId, Arc<…>>` cache + `apply_change` re-derivation with salsa's `#[salsa::input]` setters and `#[salsa::tracked]` memoization.
2. Swapping the ad-hoc `db`-shaped trait Phase 2 passed around for salsa's `&dyn Db`.
3. Moving diagnostic collection from a returned `Vec` to the `#[salsa::accumulator]` (the checker pushes; `diagnostics(file)` reads `accumulated`).
4. Interning `DefId`/`ScriptRefId`/`Name` (Phase 2 used plain ids).

The **checker body, the `Ty` enum, the binder, and the `ide` feature fns are untouched** — they already take a `db`-shaped argument and return POD. A test asserts single-file results are **byte-identical** before/after the salsa swap (no behavior change, only an engine change).

---

## Workstream 5 — Cross-file IDE features (`gdscript-ide`)

The navigation features that *need* the project index ([`research/04`](research/04-gdscript-semantics-and-features.md) §4: these are explicitly "Phase 2/3" because they need the cross-file graph). Each is a pure `(db, FilePosition|query) -> POD` fn on `Analysis`; POD lives in `gdscript-base` (serde, no `lsp-types` — [`01`](01-ARCHITECTURE.md) §2).

### 5.1 Feature → query path → POD result

| Feature | `Analysis` method | Query path | POD result |
|---|---|---|---|
| **Go-to-definition (cross-file)** | `goto_definition(pos)` | resolve token → `Symbol`/`GlobalDef`/cross-file `DefId` → its decl `FileId`+`TextRange` | `Vec<NavTarget { file, range, name, kind }>` |
| **Find-references (project-wide)** | `find_references(pos)` | resolve token → canonical `DefId`; **search candidate files** (registry + `def_map` reverse-index), confirm each same-named token *resolves to the same `DefId`* | `Vec<Reference { file, range, kind: Read\|Write\|Decl }>` |
| **Rename** | `rename(pos, new)` | find-references' confirmed set + a **safety check** (valid identifier; not a builtin/engine symbol; no resulting collision) → one edit per reference | `Result<SourceChange, RenameError>` |
| **Workspace symbols** | `workspace_symbols(query)` | `global_registry().iter()` + each `item_tree`'s top-level members, fuzzy-matched on `query` | `Vec<NavTarget>` |

### 5.2 Notes per feature

- **Go-to-definition** — extends Phase 2's (in-file) resolution across the seam: a bare `ClassName` → its `class_name` decl in another file; `preload("res://x.gd")` / `extends "res://x.gd"` → that file (head); an autoload identifier → the autoload's target file; `X.member` where `X` is a script type → the member's decl in the owning file's `ItemTree`. Returns *every* candidate when a `class_name` collides (§2.3), so the user sees the ambiguity.
- **Find-references — the dynamic-typing hazard** ([`research/04`](research/04-gdscript-semantics-and-features.md) §4, table): GDScript's gradual typing means a bare `foo` could be *many* unrelated symbols. **Correctness = resolve, don't string-match.** The algorithm: (1) compute the canonical `DefId` of the symbol under the cursor; (2) gather **candidate files** cheaply (for a global: every file; for a class member: the defining file + files that reference its class via the graph; for a local: just this function's `Body`); (3) for each same-named token in a candidate, run the resolver and keep it **iff it resolves to the same `DefId`**. This rejects unrelated same-named symbols — the difference between "find references" and "grep."
- **Rename — HARD; correctness is the bar** ([`research/04`](research/04-gdscript-semantics-and-features.md) §4: *"Very High"* difficulty; *"must be accurate incl. local scope, scenes, class_name usages"*). Rename **reuses find-references' confirmed set** (same `DefId`-equality discipline) so it inherits the no-false-positive guarantee, then layers safety checks:
  - **Conservative scope (Phase 3):** rename **locals**, **script members** (vars/consts/funcs/signals/enums), and **`class_name` globals**. **Refuse** (return `RenameError`) to rename: engine/builtin symbols (not ours to change); a symbol whose new name would **collide** in any affected scope; a `class_name` that is **also referenced by name in `.tscn`/`.tres`/`project.godot`** (Phase 3 doesn't yet rewrite scene/config files — renaming the `.gd` alone would silently break the scene → **refuse with an explanatory error** rather than corrupt). Scene-/config-aware rename is **Phase 4/6**.
  - **The contract:** rename is **correct or it refuses** — never a partial/over-broad edit. A wrong rename corrupts code; a refusal is a recoverable annoyance.

```rust
// crates/gdscript-base/src/source_change.rs  (the rename/code-action POD — shared with Phase 2 code actions)
#[derive(Serialize, Deserialize, Clone)]
pub struct SourceChange { pub edits: Vec<FileEdit> }                 // spans multiple files
#[derive(Serialize, Deserialize, Clone)]
pub struct FileEdit { pub file: FileId, pub edits: Vec<TextEdit> }   // byte-range edits within one file
#[derive(Serialize, Deserialize, Clone)]
pub struct TextEdit { pub range: TextRange, pub new_text: String }   // client converts bytes -> UTF-16

#[derive(Serialize, Deserialize, Clone)]
pub enum RenameError {
    NotRenamable { reason: String },     // engine/builtin symbol
    WouldCollide { at: FileRange, with: String },
    CrossesUnsupportedBoundary { what: String }, // class_name referenced in a .tscn/project.godot (Phase 4/6)
    InvalidIdentifier,
}
```

- **Workspace symbols** — a thin query over `global_registry` (all `class_name` + autoload names) plus each file's top-level declared members, fuzzy-ranked. The engine LSP returns `false` for `workspaceSymbol` ([`research/04`](research/04-gdscript-semantics-and-features.md) §5.1) — this is a clean differentiator.

---

## Workstream 6 — Performance & incrementality validation

The exit criterion that *defines* Tier 2 success ([`ROADMAP.md`](ROADMAP.md) Phase 3: *"editing one file does not re-type-check the whole project (measured: keystroke latency flat as project grows)"*). The risk is invalidation correctness *and* the perf it buys; both are measured, not asserted.

### 6.1 Benchmarks (criterion + a real-project fixture)

- **Large-project fixture:** a real Godot game vendored under `fixtures/projects/` (e.g. a `godot-demo-projects` title or `Maaack/Godot-Game-Template` — both already cited as real `project.godot` fixtures in [`research/04`](research/04-gdscript-semantics-and-features.md)). Aim for ≥ a few hundred `.gd` files so "flat as size grows" is meaningfully testable; include a `tiny` (10-file) and `medium` (100-file) variant to plot the curve.
- **Keystroke latency MUST stay flat as project size grows.** The headline benchmark: load the fixture, then time `apply_change(body edit) → diagnostics(edited file)` on the `tiny` / `medium` / `large` variants. **Assertion:** the latency is ~constant (within noise) across the three sizes — it must **not** scale with file count. (Cold full-project index time *may* grow with size; warm keystroke time must not.)
- **Body edit vs signature edit (the two regimes):**
  - *Body edit* (change an expression inside a `func`): recomputation set = `{parse, item_tree(unchanged→stops), body(fn), infer(fn), diagnostics(file)}` for **one** file. Measure: should be ~the Phase-2 single-file warm number (< 5 ms), **independent of project size**.
  - *Signature edit* (rename a `func`, change a param type, add/remove `class_name`): recomputation set = the edited file + its **graph dependents only**. Measure: bounded by the dependent count, **not** total file count; assert dependents-only via the event log.

### 6.2 The "editing a body doesn't invalidate globals" assertion as a test

salsa exposes an **event log** (the `salsa::EventKind::WillExecute` events for each tracked-fn execution). The invariant test:

```rust
#[test]
fn body_edit_does_not_invalidate_globals() {
    let mut host = load_fixture("fixtures/projects/medium");
    let _ = host.analysis().diagnostics(some_file);          // warm the caches
    let events = host.with_event_log(|| {
        host.apply_change(edit_a_function_body(some_file));  // change ONLY a body, not a signature
        let _ = host.analysis().diagnostics(some_file);      // re-query
    });
    // The registry / project graph / OTHER files' def_maps must NOT re-execute.
    assert!(!executed(&events, "global_registry"));
    assert!(!executed(&events, "project_model"));
    assert!(other_files_def_maps_not_executed(&events));
    // Only the edited function's body/infer (+ parse, + item_tree-validated-equal) ran.
    assert!(executed(&events, "infer") && touched_only(&events, some_file));
}
```

A companion test asserts a **signature** edit *does* re-run `global_registry` (when a `class_name` changes) and *does* re-run dependents — but **only** the graph dependents, proving the bound is the dependency graph, not the project. These two tests are **CI gates**: they are the executable form of the headline invariant.

---

## Testing strategy

1. **Cross-file resolution golden cases** (`fixtures/projects/*` multi-file): assert types/navigation across files. Cover: `extends "res://base.gd"` flattens the base's members into the subclass (member completion lists inherited script members + native members at the chain's native tail); `const X = preload("res://x.gd"); X.new()` → instance of `X`; `var v: X` resolves; bare `class_name` reference resolves project-wide; `load("res://x.gd")` literal == `preload`; `load(var)` → `Variant`; an autoload singleton `Foo.bar` resolves to `Foo`'s member; **`extends` cycle → `CYCLIC_INHERITANCE`** (and a `preload` cycle does **not** warn); a dangling `preload("res://missing.gd")` → diagnostic, not a panic.
2. **Rename correctness corpus (incl. adversarial same-name cases)** — the highest-stakes suite. For each case: `(project, cursor, new_name) → expected SourceChange | RenameError`. **Adversarial cases:** two unrelated locals named `i` in different functions (rename one, the other is untouched); a local `pos` shadowing a member `pos` (rename the local only); a method `update` on class `A` and an unrelated method `update` on class `B` (rename `A.update`, `B.update` untouched); a `class_name` referenced in a `.tscn` → **`CrossesUnsupportedBoundary`** refusal (not a corrupting partial edit); a rename that would collide with an existing symbol → `WouldCollide`. The pass bar: **zero false edits**, ever — a wrong edit is a hard failure, a refusal where a rename was possible is a soft failure (tracked, not blocking).
3. **Incremental / invalidation tests using salsa's event log** (Workstream 6.2): the body-edit-doesn't-invalidate-globals gate; the signature-edit-invalidates-dependents-only gate; a `project.godot` `[autoload]` edit re-runs `global_registry` but not unrelated `infer`s; an API-version change (HIGH durability) re-runs everything (expected, rare) while a body edit re-runs ~nothing.
4. **Multi-file fixtures** — the project fixtures double as integration fixtures for go-to-def / find-refs / workspace-symbols: assert find-references on a project-wide method is **complete** (every real call site, across files) **and correct** (no unrelated same-named symbols), and that go-to-def jumps to the right file+range for each linking mechanism.
5. **Determinism** — collisions (duplicate `class_name`) resolve to a **stable** winner across runs (deterministic file ordering); the registry and all navigation results are reproducible (no `HashMap`-iteration-order leakage into outputs).
6. **wasm32 CI gate** — `cargo check -p gdscript-db -p gdscript-hir -p gdscript-ide --target wasm32-unknown-unknown` green **with salsa pulled in** (salsa works in wasm but single-threaded — assert no thread/`Instant` APIs leak; see Risks). A wasm smoke test loads a small multi-file project from injected bytes and runs a cross-file query.
7. **Perf benchmarks** (Workstream 6.1) tracked in CI to catch latency regressions as the codebase grows.

---

## Exit criteria (mirror ROADMAP Phase 3)

A testable checklist; all must pass on a **real multi-file Godot project fixture**:

- [ ] **Rename a `class_name` updates all references across files** — every cross-file usage (type annotations, `extends`, `preload`-bound consts, `X.new()`, `is X`) is in the `SourceChange`; unrelated same-named symbols are **not** touched; a usage in a `.tscn`/`project.godot` triggers a **refusal** (no silent corruption), not a partial edit.
- [ ] **Find-references on a method is complete and correct** — every real call site across files is returned; no unrelated same-named methods/vars are included (resolved, not string-matched).
- [ ] **Editing one file does NOT re-type-check the whole project** — measured: a body edit's recomputation set excludes `global_registry`/`project_model`/other files (salsa event-log test); keystroke latency is **flat** across `tiny`/`medium`/`large` fixtures.
- [ ] **Autoload singletons complete globally** — a `*`-flagged `[autoload]` name resolves as a global identifier anywhere in the project; `Autoload.member` types correctly; a non-`*` autoload does **not** become a global.
- [ ] **`class_name` registry is source-derived** — correct even when `.godot/global_script_class_cache.cfg` is absent or stale (cache used only as a warm-start hint); duplicate `class_name` and `class_name`/autoload collisions are diagnosed deterministically.
- [ ] **Cross-file resolution lights up the Phase-2 seam** — `extends`/`preload`/`load`-literal/bare-`class_name` all resolve to real types; the only remaining `Variant` is `load(var)`; member completion + hover + diagnostics work across files with **no checker changes** beyond `resolve_external`.
- [ ] **Godot version detected** from `project.godot` `[application] config/features`, snapped to the nearest bundled `gdscript-api` minor.
- [ ] **Cancellation is real** — a concurrent `apply_change` cancels in-flight reads (salsa panic caught → `Cancellable`); the host is single-writer/multi-reader.
- [ ] **Single-file results unchanged** — Phase-2 golden outputs are byte-identical after the salsa migration (engine change, not behavior change).
- [ ] **wasm32 CI** green for `gdscript-db`/`gdscript-hir`/`gdscript-ide` **with salsa**.

---

## Risks & mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| **Invalidation bugs — stale cached results (THE risk)** | **Critical** | This is the single biggest risk in the whole roadmap ([`ROADMAP.md`](ROADMAP.md), [`research/09`](research/09-type-system-and-inference.md) §7 Tier 2: *"incrementality bugs (stale results) are the classic failure mode; budget for it"*). Mitigations: (a) let **salsa own invalidation** (red-green) — never hand-roll a memo cache; (b) the **ItemTree-excludes-bodies** invariant enforced *by construction* and **gated by the event-log test** (Workstream 6.2) so a regression fails CI; (c) golden cross-file fixtures re-run after edits to assert no staleness; (d) determinism tests so a "stale" result can't hide behind nondeterministic ordering. |
| **Stale `.godot/global_script_class_cache.cfg`** | High | **Never trust the cache** ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.1/§3.7, [`research/09`](research/09-type-system-and-inference.md) §4.2). Re-derive `class_name` globals from `.gd` source (free — we parse every file anyway); use the cache **only** to prioritize cold-start indexing, then reconcile against source truth and discard divergences. Tested with the cache absent and with a deliberately-wrong cache. |
| **Rename correctness (corrupts code if wrong)** | High | Rename **reuses find-references' `DefId`-equality** discipline (no string-matching) → no false positives; a **conservative scope** (locals/members/`class_name`; refuse engine symbols, collisions, and scene/config-crossing renames) with `RenameError` rather than a risky edit; an **adversarial same-name corpus** (Testing #2) with a *zero-false-edit* bar. Rename is **correct or it refuses.** |
| **salsa learning curve / data-model constraints** | Medium | Everything flows through `&dyn Db`; tracked structs carry a `'db` lifetime ([`research/06`](research/06-analyzer-architecture.md) §2/§8). Mitigated because Phase 2 **already** shaped every derived computation as a pure `(db, file) -> value` fn behind a `db`-trait — the migration is mechanical (Workstream 4.5), not a redesign. Pin **salsa 0.27** exactly; follow Ruff/ty's new-salsa patterns; a byte-identical single-file regression test guards the swap. |
| **Memory growth on big projects** | Medium | salsa memoizes aggressively; a few-hundred-file project holds many cached `Parse`/`ItemTree`/`InferenceResult`s. Mitigations: `Arc`-share the (HIGH-durability) `EngineApi`; intern `DefId`/`ScriptRefId`/`Name`; keep `Ty` small/`Copy`; rely on salsa LRU/GC for cold `infer` results; profile memory on the large fixture in CI and budget it. Body inference is on-demand for open/visible files ([`research/09`](research/09-type-system-and-inference.md) §4.5) — don't eagerly infer every body project-wide. |
| **Keeping it wasm-safe with salsa** | Medium | salsa works in wasm but is **single-threaded** ([`research/06`](research/06-analyzer-architecture.md) §7). Feature-gate any parallelism (`#[cfg(feature = "parallel")]`, native-only); no `std::thread`/`Instant::now`/`std::fs` in `db`/`hir`/`ide` ([`01`](01-ARCHITECTURE.md) §7); CI runs the `wasm32-unknown-unknown` check **with salsa** every PR (Testing #6). Single-writer/multi-reader degrades cleanly to single-threaded-synchronous in the browser (same code, cancellation just never fires). |
| **`res://` path mapping edge cases** (`uid://` indirection, case sensitivity, addons sub-root) | Medium | Resolve `res://` against the `project.godot` directory; treat `uid://` as Phase 4 (scene-era) work; keep path comparison consistent with Godot (case-sensitive `res://` keys). `res://addons/*` is a gating sub-root only (`exclude_addons`), same resolution domain. Dangling paths → a diagnostic, never a panic. |
| **Cycle handling** (`extends` cycles illegal; `preload` cycles legal) | Low-Med | The cross-file member-lookup cursor caps depth and emits `CYCLIC_INHERITANCE` for `extends` cycles ([`research/09`](research/09-type-system-and-inference.md) §4.4); `preload`/`load` cycles are **not** reported (legal at runtime). Both covered by golden fixtures (Testing #1). |

---

## References (relative links)

- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — crate stack (§1), `AnalysisHost`/`Analysis` API + cancellation (§2), **salsa-adopted-at-Tier-2** (§3), FFI/WASM (§4), data model + multi-version selection (§5), **portability rules / wasm-safe** (§7).
- [`ROADMAP.md`](ROADMAP.md) — Phase 3 deliverable + exit criteria; Tier-2 placement; the "biggest risk = project-wide incremental invalidation" framing; the dependency graph.
- [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) — what single-file already does; the **`resolve_external → Unknown` seam** this phase fills; the `Ty::Unknown`/`Ty::ScriptRef` variants; the pure `(db,file)->value` fns this phase converts to tracked queries; the `SourceChange` POD reused by rename.
- [`PHASE-1-PARSER-AND-SYNTAX-MVP.md`](PHASE-1-PARSER-AND-SYNTAX-MVP.md) — the CST/AST + `AnalysisHost` skeleton + `Cancellable<T>` this phase makes real.
- [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) — the `res://` convention, the `Change` type, multi-version `gdscript-api` bundling/selection, the project fixtures.
- [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) — consumes the VFS's `.tscn`/`.tres` files (already ingested here) to sharpen `$Path`/`%Unique`/`get_node` from `Node` to the concrete node class; adds scene-aware rename.
- [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) — the full 48-warning set on top of this phase's gating layer; real CFG flow narrowing; scene/config-aware rename.
- [`GODOT-SYNC.md`](GODOT-SYNC.md) — the multi-version `gdscript-api` pipeline whose version selection this phase wires to `project.godot` detection.
- [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) — **PRIMARY**: salsa 0.27 (input/tracked/interned/accumulator, red-green, durability), `AnalysisHost`/snapshot, cancellation → `Cancellable<T>`, single-writer/multi-reader threading, the MVP→v1 migration path.
- [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) — **PRIMARY**: the project graph (§4), the `class_name` registry / cache-is-a-hint (§4.2), autoloads (§4.3), `preload`/`load`/`extends` edges (§4.4), `DefMap`, the **incremental invariant** (§3.1, §4.5), the merged script+native `Ty` lattice + `is_assignable` (§3.3, §6).
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — **PRIMARY**: the five linking mechanisms (§3), `project.godot` format + `[autoload]` + the `*` flag (§3.3, §3.6), `class_name` registry + `.godot/global_script_class_cache.cfg` caveats (§3.1, §3.7), `res://` resolution (§3.2), the IDE-feature → required-data table incl. find-refs/rename difficulty (§4), the engine-LSP gaps this phase fills (§5).
