# gdscript-analyzer Phase 3 — Execution Playbook (Project-Wide Resolution & Incremental)

> **Status:** execution playbook. **Tier:** 2. **Reconciles:** [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) (the design plan, written pre-Phase-2) with a **corpus-grounded research pass** run against the *implemented* Phase-2 tree (verified `crates/**` `file:line`), the ranked real-project demand (`ReactiveUI-Gadot`, 89 `.gd`), and primary-source-verified salsa / Godot-4.4 facts.
> **What changed vs the plan:** the plan's design holds; this playbook (a) re-orders the work into **five corpus-validated, independently-shippable milestones** (M0–M5) instead of six parallel workstreams, (b) pins everything to **real `file:line` seams** instead of sketches, (c) folds in **corrections** the implemented code and re-verified sources forced (salsa provenance, durability nuance, `load()` opacity, the autoload `*` flag, the Godot-4.4 precedence order). See §8 for the full diff.
> **The one rule, unchanged:** Phase 3 reimplements **only** `resolve_external` (now backed by salsa queries over the project graph) and **activates `Ty::ScriptRef`**. The checker body, the `Ty` lattice, the binder, and every `ide` feature fn are untouched. Resolution is **monotone**: known → informative type; not-yet-known → `Ty::Unknown` (never a false diagnostic).

---

## 0. The seam, exactly (verified against the implemented tree)

Everything below is confirmed against the working tree (branch `feat/phase-2`, identical to the merged Phase-2 base).

**The single seam — `crates/gdscript-hir/src/resolve.rs:41`:**
```rust
pub fn resolve_external(_r: &ExternalRef) -> Ty { Ty::Unknown }
```
- `ExternalRef` (`resolve.rs:24-34`) has **four** variants: `ClassName(SmolStr)`, `ExtendsPath(SmolStr)`, `Preload(SmolStr)`, `Autoload(SmolStr)`.
- **Live callers (all inside `resolve.rs`, no cross-crate callers):** `:109` (bare unknown type annotation → `ClassName`), `:121` (dotted non-`Class.Enum` annotation → `ExtendsPath`), `:169` (`extends <non-engine Name>` → `ClassName`), `:172-174` (`extends "res://…"` / `extends A.B` → `ExtendsPath`).

**Gaps that BYPASS the seam today (must be routed back through it):**
- `ExternalRef::Preload` and `ExternalRef::Autoload` are **declared but never constructed** anywhere. `preload(...)` short-circuits to `Ty::Unknown` at `infer.rs:642-648` (`Expr::Preload` arm); a bare autoload name falls through `resolve_name` to the catch-all `Ty::Unknown` at `infer.rs:1077`.
- `class_name` **is lowered** into `ItemTree.class_name` (`item_tree.rs:25,208-211`) but **has no consumer** — there is no global index, so another file's `class_name` arrives as an unknown bare name → `ClassName` → `Unknown`.
- `is`/`as` against a user type: `Expr::Is`/`Expr::Cast` (`infer.rs:585-592`) → `resolve_ptr_ty` → `resolve::resolve_type_ref` (`infer.rs:1152-1155`) → `resolve_external(ClassName) → Unknown`; `apply_narrowing` only records a fact when `!narrowed.is_uninformative()` (`infer.rs:1131-1134`), so the narrowing is **silently dropped**.

**Why it is sound today, and must stay sound under *partial* resolution (the load-bearing invariant):** member access **propagates the seam** — a field of an `Unknown` receiver is `Unknown` (`infer.rs:836-841`); `is_uninformative() = Variant|Unknown|Error` (`ty.rs:97-100`) suppresses every `UNSAFE_*`/`INFERENCE_ON_VARIANT`; `is_assignable` returns `Ok` whenever `Unknown`/`Error`/`ScriptRef` is on either side (`ty.rs:189-191,227,276`). **Invariant:** any *unresolved* user ref falls back to `Unknown`, never `Error` and never a half-built type. This is what keeps cross-file code at **zero false diagnostics** while we light the seam up one construct at a time.

**The dormant target — `crates/gdscript-hir/src/ty.rs:42`:** `Ty::ScriptRef(ScriptRefId)` already exists and is already treated as assignable-to-anything and hover-elided. Activating it (giving `ScriptRefId` a resolved base + member table) is M1's core and **does not perturb existing inference**.

**The signature ripple (the single largest mechanical task — see §2 decision D2):** `resolve_external` is currently context-free. Threading a db/resolver handle ripples to `resolve_type_ref`, `resolve_type_name`, `resolve_base`, `ClassScope::new` (`resolve.rs:189-246`), `analyze_file` (`infer.rs:208-261` — today a pure `(api, root) -> FileInference` with **no `FileId`, no db**), and every `resolve_type_ref` call site in `infer.rs`/`semantic.rs`.

---

## 1. Confirmed dependency table

### Runtime dependencies (enter the shipped graph; must stay wasm32-clean)

```toml
# --- the incremental engine (new "salsa 2022", macro-based) ---
salsa        = "0.27"      # CONFIRMED latest 0.27.1; re-check for 0.28+ before locking
salsa-macros = "0.27"      # pinned to salsa's exact minor

# already present, reused:
# rustc-hash (FxHashMap), smol_str/ecow (SmolStr/EcoString), triomphe/Arc, serde
```

- **CONFIRMED (3/0):** adopt **upstream crates.io `salsa` 0.27**, the macro-driven rewrite (`#[salsa::input]`, `#[salsa::tracked]`, `#[salsa::interned]`, `#[salsa::accumulator]`, `#[salsa::db]`, `#[derive(salsa::Update)]`, `salsa::Supertype`). **DO NOT** design against the legacy `salsa::query_group!`/`database_impl!`/`ParallelDatabase::snapshot` API — it is dead upstream and survives only as rust-analyzer's vendored `ra-salsa` fork.
- **CONFIRMED:** rust-analyzer itself is on **upstream** new salsa (crates.io `salsa 0.27.0`, features `rayon, salsa_unstable, macros, inventory`) as of 2026-06 — so RA's `base-db` idioms copy cleanly onto the same crate. *(Provenance corrected from the design plan: the migration shipped via PR **#18964** merged 2025-03-10, in the 2025-03-17 release; RA first depended on salsa **0.20**, reaching 0.27 only by 2026-06. None of this changes the decision — see §8/R5.)*
- **wasm rule (unchanged, now a hard gate):** salsa works on `wasm32-unknown-unknown` but **single-threaded**. No `std::thread`/`Instant::now`/`std::fs`/`notify`/`rayon` may leak into `gdscript-db`/`gdscript-hir`/`gdscript-ide`. Parallelism + file watching live in a **native-only loader crate** (§3.M0, §4.4). The `cargo check --target wasm32-unknown-unknown` gate runs **with salsa pulled in**, every PR.

### Build / dev dependencies (never shipped, never wasm)
```toml
# native-only loader (file discovery + watching), feature-gated, native targets only:
notify   = "…"   # debounced watcher; SECONDARY change signal (editor LSP events are primary)
walkdir  = "…"   # project enumeration
# both confined to the loader crate; excluded from the wasm32 gate.
```

---

## 2. Resolved architecture decisions

The design plan left nine open questions; the research pass resolved each into a decision. Decisions marked **CONFIRMED** were verified against primary sources or the implemented code; **DECIDED (default)** are the recommended answer adopted for the playbook, revisitable at the M1 spike.

- **D1 — salsa version & query granularity. CONFIRMED.** salsa 0.27; the layering is RA's body-edit-firewalled stack, simplified:
  ```
  file_text(FileId)      #[salsa::input]    ← VFS leaf, keyed by externally-assigned FileId(u32)
     → parse(file)        #[salsa::tracked]  ← CST/AST (gdscript-syntax)
     → item_tree(file)    #[salsa::tracked]  ← FIREWALL: items only, NO bodies (item_tree.rs:5-9)
     → global_registry()  #[salsa::tracked]  ← class_name + autoload index (NEW; keyed on item_tree slots only)
     → def_map(file)      #[salsa::tracked]  ← per-file scope + this file's cross-file resolutions (NEW)
     → infer(file, fn)    #[salsa::tracked]  ← existing analyze_file, re-parameterized (D2)
  ```
  **Two firewalls carry the whole perf story:** `item_tree` must stay body-edit-stable (the existing `item_tree.rs:5-9` invariant), and `global_registry` is keyed on `item_tree` `class_name`/`extends` slots only, never on bodies. **Key-hygiene (CONFIRMED, RA `guide.md`): never key a query on a text offset, range, or syntax node** — use per-file positional ids (the `ItemTree` slot / an `AstId` analog). Offsets shift on every keystroke and would defeat every firewall.

- **D2 — resolver threading (blocks M1). DECIDED (default): thread `&dyn Db` + `FileId` into `analyze_file` and downward.** `resolve_external` becomes `resolve_external(db: &dyn Db, from: FileId, what: &ExternalRef) -> Ty`. `analyze_file` (`infer.rs:208-261`) gains `db: &dyn Db, file: FileId`; `ClassScope::new`/`resolve_base`/`resolve_type_ref`/`resolve_type_name` thread the same. **Spike this ripple FIRST in M0** (it is mechanical but wide) and confirm before M1 lands behavior. The byte-identical single-file regression (§5) guards it.

- **D3 — user-class `Ty` representation. DECIDED (default): activate the dormant `Ty::ScriptRef(ScriptRefId)` (`ty.rs:42`).** `ScriptRefId` indexes a slot in `global_registry` (interned). The resolved shape `{ base: Ty, members: Map<Name, Ty> }` is produced **lazily by a tracked query** (`script_class(db, ScriptRefId)`), not stored inline in `Ty` (keeps `Ty` small/`Copy`-ish and avoids cycles in the type itself). Extend `is_assignable`/`label`/`lookup_member` for `ScriptRef`, mirroring the native surface in `gdscript-api/src/lookup.rs:126-200`. **The base walk must cross from a user base into an engine base** (a user class `extends RefCounted` resolves inherited members through the engine `lookup_member`).

- **D4 — `class_name` global index: build, invalidation, collisions. CONFIRMED.** `global_registry()` scans every file's `item_tree` for `class_name`+`extends` (free — every file is parsed anyway), keyed **strictly on `item_tree` slots** (body-stable). **One flat project-wide namespace; no folder scoping; no imports.** Collisions (duplicate `class_name`; a `class_name` shadowing a builtin/native/autoload) emit Godot's **`Class "%s" hides …`** diagnostic and keep the **first by deterministic file order** as resolvable — never panic, stable across runs.

- **D5 — `preload`/`load` ingestion + the wasm rule. CONFIRMED + DECIDED.** Route the currently-unused `ExternalRef::Preload`/`Autoload` **back through `resolve_external`** (keeps "one seam"; M3/M4 finally construct them). `project.godot` is **injected as text** via the loader (the wasm-clean core forbids `std::fs`), exactly like a `.gd` file. **`load()` stays opaque** — only `preload`/`const`/`class_name`/autoload bases permit static `.new()`/`.CONST`/`.Inner` (§6 spec). Add `LoadLiteral`/`LoadDynamic` handling **in `infer.rs`** (the `Expr::Call` arm for `load`), not as `resolve_external` variants, because `load`'s opacity is an inference concern, not a name-resolution one. *(This is the one place the playbook diverges from the plan's sketch — see §8.)*

- **D6 — `is`/`as` narrowing depth. DECIDED (default).** Once `resolve_type_ref` yields informative user types, removing the `is_uninformative` guard at `infer.rs:1131-1134` lets `apply_narrowing` record user-class facts. M4 also adds **user-class subtype semantics** to narrowing (`is` against a user *base* narrows to the base; against the exact class narrows to it) — reuse `is_subclass` over `ScriptRef`.

- **D7 — cycle ownership. DECIDED (default): hand-roll Godot-faithful cycle *detection* for correct diagnostics; let salsa own *invalidation*.** Mirror Godot's `RESOLVING` sentinel + the post-resolution fqcn walk (§6.3) for the `CYCLIC_INHERITANCE` diagnostic on `extends`; **`preload` cycles are legal at runtime and must NOT warn** (shallow-load tolerance). salsa's red-green replaces Godot's hand-rolled inverse-dependency map for *invalidation*.

- **D8 — `global_script_class_cache.cfg`: hint only, never truth. CONFIRMED.** Derive `class_name` truth from `.gd` source; use `res://.godot/global_script_class_cache.cfg` **only** to order cold-start indexing. Gated by absent-cache and deliberately-wrong-cache fixtures (§5).

- **D9 — guitkx generated `.gd`. DECIDED (default): treat generated `.gd` as ordinary source** (it carries a real `class_name`) and **defer `.guitkx`-aware go-to-def mapping** to a later client phase. The `preload`-const vs `class_name`-global dual channel is unified by the registry (both land on the same `ScriptRefId`).

- **D10 — second validation corpus (blocks M2/M4). DECIDED.** The reference corpus exercises **0** user-`extends` and **0** autoloads. I will source a public Godot-4 demo project (inheritance chains + `*`/non-`*` autoloads) and wire it into the corpus harness **before M2 ships**. Until then, M2/M4 are validated by synthetic fixtures only.

---

## 3. Per-milestone implementation notes (M0–M5)

Milestone-ordered (each independently shippable + corpus-gated), mapping the plan's Workstreams onto the corpus-ranked priority. **M1 alone delivers ~85 % of real demand.** Each milestone leaves the tree green and noise-free.

### Corpus demand (the ordering rationale)
From `ReactiveUI-Gadot` (89 `.gd`, `config/features="4.7"`): global `class_name` references **~850+** (`V.` 483, `Hooks.` 219, `RUIVNode.` 29, `RUIConfig.` 26, `ReactiveRoot.` 20) ≫ typed declarations **149** ≫ `class_name` decls **67** (= the 67 cache entries) ≫ `extends` **89** (73 `RefCounted`, 14 `SceneTree`, 2 `Control`, 1 `EditorPlugin`; **0 user, 0 path**, 1 inner) ≫ `preload` **24** + `load` **6** ≫ `is`/`as` user **16** ≫ autoloads **0** ≫ scene/node **0**.

---

### M0 — Salsa substrate + VFS migration (foundation; ZERO behavior change)

**Plan mapping:** Workstream 4 (salsa) + Workstream 1's VFS half. **Crates:** `gdscript-db` (fill the stub `db/src/lib.rs:10-16`), `gdscript-ide` (`lib.rs:33-99`).

**Move the VFS down into `gdscript-db`** (today `AnalysisHost` owns `files: Arc<FxHashMap<FileId, Arc<str>>>` at `gdscript-ide/src/lib.rs:33-36`; single mutation point `apply_change` at `:71-83`; cloned `Analysis` snapshots at `:87-99`; whole-file reparse at `:101-227`).

```rust
// crates/gdscript-db/src/lib.rs  (sketch — the salsa declarations)
#[salsa::input]
pub struct FileText {
    #[returns(ref)] pub text: Arc<str>,
    pub file_id: FileId,                 // the existing opaque FileId(u32) (base/lib.rs:14-17)
}
#[salsa::input]
pub struct ProjectConfig { #[returns(ref)] pub project_godot_text: Arc<str> }   // injected text (D5)
#[salsa::input]
pub struct ApiInput { pub version: ApiVersion, #[returns(ref)] pub api_bytes: Arc<[u8]> }

#[salsa::accumulator] pub struct Diagnostics(pub Diagnostic);  // replaces returned Vec<Diagnostic>

#[salsa::tracked] pub fn parse(db: &dyn Db, f: FileText) -> Parse { /* gdscript-syntax */ }
#[salsa::tracked] pub fn item_tree(db: &dyn Db, f: FileText) -> Arc<ItemTree> { /* signatures only */ }
#[salsa::tracked] pub fn analyze_file(db: &dyn Db, f: FileText) -> Arc<FileInference> { /* re-param (D2) */ }

#[salsa::db] #[derive(Default, Clone)]
pub struct RootDatabase { storage: salsa::Storage<Self> }
#[salsa::db] impl salsa::Database for RootDatabase {}
```

Steps:
1. Add `salsa 0.27`. Define `FileText`/`ProjectConfig`/`ApiInput` inputs; `#[derive(salsa::Update)]` on every stored return (`ItemTree`, `FileInference`, later the registry + user member tables).
2. Turn `parse`/`item_tree`/`analyze_file`/the semantic features into `#[salsa::tracked]` fns taking `&dyn Db`.
3. **`AnalysisHost::apply_change` becomes the single salsa input-setter** (preserves the existing single-writer discipline). `Analysis` snapshots become **cloned db handles** (new salsa: the handle is `Clone + Send`; no `snapshot()`). Public `gdscript-ide` API shape unchanged.
4. **Durability tiers** (D-table): `ApiInput` **HIGH**, `ProjectConfig` **MEDIUM**, edited `FileText` **LOW**. Set via `set_*_with_durability`. ⚠️ Mis-tiering is silently catastrophic (engine API at LOW re-validates the whole subgraph per keystroke).
5. **Wire real cancellation** into the already-present `Cancellable` surface (`base/lib.rs:311-314`, `lib.rs:94-95`). New-salsa model: `&mut db` input mutation cancels outstanding queries on cloned handles (they unwind via the `Cancelled` panic). Long tracked fns call **`db.unwind_if_revision_cancelled()`** (note the rename from `unwind_if_cancelled`, salsa #447). The `ide` boundary catches with `salsa::Cancelled::catch(...)`. **Never mutate inputs while holding a clone on the same thread** (deadlock).
6. **The `resolve_external` threading spike (D2):** land the `&dyn Db, FileId` ripple now, with `resolve_external` still returning `Unknown` — so M0 is provably zero-behavior-change.

**Gate (BLOCKING):** all Phase-2 tests pass **byte-identical**; the **`body_edit_does_not_invalidate_globals`** salsa-event-log test (permanent CI gate, §5); `cargo check --target wasm32-unknown-unknown` green **with salsa**; a memory-budget check on the reference corpus (see R1).

---

### M1 — Global `class_name` resolution + user-class typing (THE milestone — ~85 % of demand)

**Plan mapping:** Workstream 2 (registry) + Workstream 3.3 (`resolve_external(ClassName)`) + the `Ty::ScriptRef` activation. **Files:** `gdscript-db` (new `global_registry()`), `resolve.rs:41` (reimplement `ClassName`), `ty.rs:42` (activate `ScriptRef`), `infer.rs` (member access on a `ScriptRef` receiver).

**(A) The global registry** (mirrors `ScriptServer`'s `HashMap<StringName, GlobalScriptClass>`):
```rust
// crates/gdscript-db/src/global_registry.rs
#[salsa::tracked]
pub fn global_registry(db: &dyn Db) -> Arc<GlobalRegistry> { /* scan every item_tree */ }

pub struct GlobalRegistry {
    classes:   FxHashMap<SmolStr, GlobalClass>,  // class_name -> {file, script_ref, base, native_base}
    autoloads: FxHashMap<SmolStr, Autoload>,     // filled in M4
    collisions: Vec<GlobalCollision>,            // surfaced via the accumulator (D4)
}
```
Built from `{ item_tree(f) | f in project }` `class_name`/`extends` slots → body-edit-stable. Single-pass; no fixed-point loop (no macros — see §8 note "do not restore RA parity").

**(B) Light up `resolve_external(ClassName)`:**
```rust
ExternalRef::ClassName(name) => match global_registry(db).classes.get(name) {
    Some(gc) => Ty::ScriptRef(gc.script_ref),     // was Ty::Unknown
    None     => Ty::Unknown,                       // truly undefined → stays Unknown (M-invariant)
},
```

**(C) Activate `Ty::ScriptRef`** with a lazily-resolved member/base table:
```rust
#[salsa::tracked]
pub fn script_class(db: &dyn Db, sref: ScriptRefId) -> Arc<ScriptClass> {
    // { base: Ty (resolved extends), members: Map<Name, Ty> (own item_tree members) }
}
```
Extend `is_assignable`/`label`/`lookup_member` (`ty.rs`) for `ScriptRef`. Member access on a `ScriptRef` receiver (`infer.rs:831-841`) resolves via `script_class(db, sref).members` then walks `base` (M2 completes the cross-realm walk; M1 handles own-members + a native base).

**Precedence (CONFIRMED, Godot 4.4 `reduce_identifier` 4319–4617):** locals/params/members ≫ class members + bases ≫ **builtin Variant type** ≫ **native engine class** ≫ **`class_name` global** ≫ autoload ≫ constants/utilities. M1 inserts the `class_name` tier *after* builtin+native (so `class_name Node` never shadows the engine `Node` in type position — D4 diagnoses it).

**Gate (reference corpus):** `V.fc`, `Hooks.use_state`, `DemoBox.render` resolve to real methods; the 149 typed declarations get informative types; `V.`/`Hooks.` member completions work; **go-to-definition crosses files** (drop the single-file caveat at `base/lib.rs:295-297`); cache-absent + cache-wrong fixtures pass; **0 false diagnostics** sustained on the full corpus.

---

### M2 — `extends` user-class + base-chain inheritance

**Plan mapping:** Workstream 3.1 (`extends`) + 3.4 (cross-file member walk). **Files:** `resolve.rs:163-176` (`resolve_base`/`resolve_external(ExtendsPath)`), the user→engine base walk.

- `extends MyGlobalClass` → `global_registry().classes` → that file's `ScriptRef` base.
- `extends "res://x.gd"` → `res_path → FileId` table → its `ScriptRef`. `extends "res://x.gd".Inner` → the inner-class slot.
- `extends Name` resolution order (CONFIRMED, `resolve_class_inheritance` 344): (1) `class_name` global; (2) autoload that is a GDScript; (3) native `class_exists` (reject engine singletons); (4) current-scope inner/sibling classes incl. a const member bound to a preloaded script. **No `extends` → `RefCounted`.**
- **The two-realm walk:** `lookup_member` over a `ScriptRef` walks user `item_tree`s up the `extends` chain, then **crosses into `EngineApi::lookup_member` at the first native base** — one unified walk. `super`/`super.method()` resolves against a script base (was `Unknown`).
- **Cycle handling (D7):** `RESOLVING` sentinel during base resolution → `CYCLIC_INHERITANCE`; post-resolution fqcn walk up `base.class_type`. `extends` cycles are errors; `preload` cycles are **not**.

**Gate:** synthetic fixtures for cross-file user inheritance + `extends "res://…"` + extends cycles (⚠️ the reference corpus has **0** user-extends → M2 validation **requires the second corpus, D10**); inherited-member lookup + `self.member` resolve through a user base.

---

### M3 — `preload`/`load` const-aliasing

**Plan mapping:** Workstream 3.2. **Files:** `infer.rs:642-648` (route `Expr::Preload` through `resolve_external(Preload)` instead of short-circuiting), `resolve.rs` (`ExternalRef::Preload` finally constructed), the `Expr::Call` `load` arm (D5).

- `const X = preload("res://x.gd")` → constant-fold the path → `res_path → FileId` → a **SCRIPT meta-type** (`is_meta_type`, `is_constant`; CONFIRMED `reduce_preload` 4647 / `make_script_meta_type` 116). Then `X.new()` → an **instance** of the class, `X.CONST` → the constant member, `X.Inner` → the inner meta-type, `var v: X` / `v is X` resolve.
- **`load("res://x.gd")` literal vs `load(var)`:** `load` is an ordinary runtime call returning an opaque `Resource`/`Object` — **NOT constant, NOT a meta-type** (CONFIRMED — this corrects the design plan, §8). `load(...).new()` is **not** statically typed. Treat `load()` as opaque (`Unknown`/bare `Object`) in the `Expr::Call` arm; **do not** alias it to `preload`.
- Cyclic-preload tolerance via shallow load (legal at runtime — no diagnostic).

**Gate (reference corpus):** `const F = preload("…/fiber.gd")` then `F.new()`/`F.tag_for_vnode(...)` typed; the 24 preloads resolve; the 6 `load(...)` produce **no** false diagnostics and **no** false static typing.

---

### M4 — Autoloads + `is`/`as` user narrowing

**Plan mapping:** Workstream 1.2 (`project.godot` parse) + 2.4 (autoload globals) + the narrowing fix. **Files:** `project.godot` `[autoload]` parser → `GlobalRegistry.autoloads`, `resolve.rs` (`ExternalRef::Autoload` constructed), `infer.rs:1131-1134` (drop the `is_uninformative` guard; add user subtype narrowing).

- **`project.godot` parsing (ConfigFile/INI, typed-Variant values):** parse `[application] config/features` (first version-shaped entry = engine minor; CONFIRMED `"4.7"` in the corpus) and `[autoload]`. **`[autoload] Name="*res://path"` — the leading `*` is the GLOBAL/singleton flag** (CONFIRMED `project_settings.cpp`: `begins_with("*")` → `is_singleton=true`, then `substr(1)` strips it). **Strip `*`; seed the global-name table ONLY for `*`-flagged entries.** No `*` → loads at `/root/Name`, **not** a global. ⚠️ The web claim "`*` = disabled" is **inverted/wrong** — reject it (§8).
- **Autoload type (CONFIRMED `reduce_identifier` 4507–4546):** a global value identifier, ≥ `Node`. Script autoload → typed as the script's instance (`ScriptRef`); `PackedScene` autoload → script type only if the scene root carries a `class_name`/global-constant script, else `Node`. `is_constant=true`. (Scene-root sharpening is Phase 4.)
- **`is`/`as` narrowing (D6):** with informative user types flowing, drop the `!is_uninformative` guard; `is UserBase` narrows to the base, `is ExactClass` to the class (via `is_subclass` over `ScriptRef`).

**Gate:** ⚠️ **needs the second corpus (D10)** — reference has 0 autoloads. `is RUIVNode`/`as RUIRouterLocation` narrowing recorded; autoload `.member` typed (script vs scene per §6).

---

### M5 — Cross-file navigation (the project-index features)

**Plan mapping:** Workstream 5. **Files:** `gdscript-ide` (`Analysis` methods + `def_map` reverse-index), `gdscript-base` `SourceChange`/`RenameError` POD. Go-to-def already shipped in M1; M5 adds find-references, rename, workspace symbols. (ROADMAP Phase-3 exit criteria require these.)

| Feature | `Analysis` method | Correctness rule | POD |
|---|---|---|---|
| **Find-references (project-wide)** | `find_references(pos)` | **resolve, don't string-match**: compute the canonical `DefId`; gather candidate files (global → all; member → defining file + graph referrers; local → this `Body`); keep a same-named token **iff it resolves to the same `DefId`** | `Vec<Reference>` |
| **Rename** | `rename(pos, new)` | reuse find-refs' confirmed set + safety: valid ident; not a builtin/engine symbol; no resulting collision; **refuse** (`RenameError::CrossesUnsupportedBoundary`) if the `class_name` is referenced in a `.tscn`/`project.godot` (Phase 4/6 rewrites those) | `Result<SourceChange, RenameError>` |
| **Workspace symbols** | `workspace_symbols(query)` | `global_registry().iter()` + each `item_tree`'s top-level members, fuzzy-ranked | `Vec<NavTarget>` |

**The rename contract: correct or it refuses** — never a partial/over-broad edit. The adversarial same-name corpus (§5) has a **zero-false-edit** bar.

**Gate:** rename a `class_name` updates every cross-file usage (annotations, `extends`, `preload`-bound consts, `X.new()`, `is X`); a `.tscn` usage triggers a refusal, not corruption; find-refs on a project-wide method is complete + correct (no unrelated same-named symbols).

---

## 4. The salsa migration deep-dive (the riskiest piece)

The analog of Phase 1's cstree deep-dive — get this wrong and either incrementality is incorrect (stale results) or latency scales with project size.

### 4.1 The two firewalls (the entire perf story)
- **`item_tree` body-stability.** A body edit re-runs `parse` and re-derives `item_tree`, but salsa back-dates the *equal* `item_tree` value → `global_registry`, other files' `def_map`/`infer` do **not** recompute. This is enforced *by construction* (the `item_tree.rs:5-9` no-bodies rule) and **gated by an event-log test** (§5). If a future change makes `item_tree` read a body, the invariant breaks silently — hence the test is a permanent CI gate, not a nicety.
- **`global_registry` body-independence.** Keyed on `class_name`/`extends` slots only. A signature edit that adds/removes a `class_name` *does* re-run it; an ordinary body edit cannot.

### 4.2 Durability is the latency-flatness mechanism
salsa keeps a per-durability revision vector. A query that read **only MEDIUM-or-higher** inputs is validated in O(1) when only a LOW input changed (CONFIRMED nuance: medium-or-higher, **not** strictly HIGH — so MEDIUM project config still benefits; corrects the plan's binary HIGH/LOW framing, §8). This is *why* keystroke latency stays flat as the project grows: the bulk (other files' signatures, the stdlib API, project config) is higher-durability than the one file under the cursor.

### 4.3 Memory — budget for it up front (R1, HIGH)
RA's salsa port **roughly quadrupled memory** (≈5–6 GB → 22–30 GB, issue #19402, bisected to the port commit) + startup regressions (#19404), from per-tracked-fn memo/sync tables and interned-struct overhead. Mitigations, applied from M0:
- Keep tracked-fn count and interning churn **low**; only intern values genuinely hot for equality (`ScriptRefId`, `Name`).
- Prefer `#[returns(ref)]` + deref over cloning stored values.
- Use `#[salsa::accumulator]` for diagnostics — do **not** thread error `Vec`s through returns.
- Don't create tracked structs whose memo tables you never read.
- Don't eagerly `infer` every body project-wide — infer on demand for open/visible files.
- **Measure memory on the reference corpus each milestone** (a CI budget check).

### 4.4 Cancellation + threading (single-writer / multi-reader)
- The owner (LSP main loop) applies changes on one thread; reads run on **cloned db handles**. A newer keystroke's `&mut db` mutation cancels in-flight reads (they unwind via `Cancelled`); the `ide` boundary catches → `Cancellable::Err`; the client re-issues against the fresh handle.
- **wasm:** single-threaded, synchronous — `Cancelled::catch` still compiles; cancellation simply never fires mid-query (no concurrent writer). Same code path, no thread APIs compiled in.
- **File discovery + watching live OUTSIDE the core** (CONFIRMED RA model: the VFS is a pure in-memory change log; "VFS doesn't do IO or file watching itself"). A **native-only loader crate** does `walkdir` discovery + debounced `notify` watching and pushes a `Change` into `apply_change`. **Primary change signal = editor LSP `didOpen`/`didChange`/`didSave`**; `notify` disk-watching is the **secondary** fallback for out-of-editor edits. (RA itself calls its watching code "untested and quite probably buggy" and defaults to editor-driven watching — so do we.)

### 4.5 `res://` ↔ FileId (keep absolute paths out of the core)
Resolve `res://` paths relative to the `project.godot` anchor via a `res_path → FileId` table in `gdscript-db` (RA's `AnchoredPath` rationale: forbid absolute paths in the core). `load(var)` and absolute OS paths never enter the wasm-clean core. `res://` = the directory containing `project.godot`; discovery walks up from any opened `.gd`; **no `project.godot` → single-file mode, never an error.** Honor `.gdignore` (empty marker → prune the folder; **`.gdignore`** is the documented name, **not** `.godotignore`).

---

## 5. Test & fixtures infrastructure

1. **Byte-identical single-file regression (M0):** every Phase-2 golden output is identical after the salsa swap (engine change, not behavior change). The guard for the D2 ripple.
2. **The invariant gates (salsa event log) — BLOCKING CI:**
   - `body_edit_does_not_invalidate_globals`: warm caches, edit only a body, assert `global_registry`/`project_model`/other files' `def_map` did **not** re-execute; only the edited fn's `infer` (+ parse, + item_tree-validated-equal) ran.
   - `signature_edit_invalidates_dependents_only`: a `class_name`/param-type edit re-runs `global_registry` + **graph dependents only**, not the whole project.
   - `project_godot_autoload_edit`: re-runs `global_registry`, not unrelated `infer`s.
3. **Cross-file resolution golden cases** (multi-file fixtures): `extends "res://base.gd"` flattens base members; `const X = preload(...); X.new()` → instance; bare `class_name` resolves project-wide; **`load(var)` → opaque, `load("lit")` → opaque** (D5 — both opaque, unlike the old plan); `extends` cycle → `CYCLIC_INHERITANCE`; `preload` cycle → **no** warning; dangling `preload("res://missing.gd")` → diagnostic, not panic.
4. **Cache-is-a-hint fixtures:** the registry is correct with `global_script_class_cache.cfg` **absent** and with a **deliberately-wrong** cache.
5. **Rename adversarial corpus (zero-false-edit bar):** two unrelated locals `i`; a local shadowing a member; `A.update` vs unrelated `B.update`; a `class_name` referenced in `.tscn` → refusal; a colliding rename → `WouldCollide`.
6. **Second corpus (D10):** a public Godot-4 demo with user-`extends` chains + `*`/non-`*` autoloads, wired into `cargo run -p gdscript-ide --example corpus` — **required before M2 ships**; the reference `ReactiveUI-Gadot` remains the zero-false-diagnostic baseline.
7. **wasm32 CI gate** green for `gdscript-db`/`gdscript-hir`/`gdscript-ide` **with salsa** (assert no thread/`Instant`/`fs` leak).
8. **Perf benchmarks** (criterion): keystroke latency **flat** across tiny/medium/large fixtures; body-edit warm time ≈ the Phase-2 single-file number (< 5 ms), independent of project size.

---

## 6. Godot 4.4 resolution spec (what the resolver returns)

Faithful to `modules/gdscript/gdscript_analyzer.cpp` (pin the engine tag; re-diff per release). Represent types with a `DataType` analog `{ kind: BUILTIN|NATIVE|SCRIPT|CLASS|ENUM|VARIANT, is_meta_type, is_constant, native_type, script_ref }`. **`is_meta_type` is load-bearing** — it distinguishes "the class" (`.new()`, statics, `.Inner`) from "an instance."

- **`class_name X`** → CLASS/SCRIPT **meta-type** (`is_meta_type`, `is_constant`). `X.new()` → instance; `X.CONST` → constant member; `X.Inner` → inner meta-type. Shadowing → `Class "%s" hides a built-in type / native class / global script class / autoload singleton`.
- **Autoload (bare name)** → global value ≥ `Node` (§3.M4).
- **`preload("res://x.gd")`** → compile-time **CONSTANT** SCRIPT meta-type (`make_script_meta_type` 116). `const X = preload(...)` makes `X.new()`/`X.CONST`/`X.Inner` static.
- **`load(...)`** → ordinary runtime call → opaque `Resource`/`Object`, **not** constant, **not** meta. **Opaque to the resolver.**
- **`extends`** (`resolve_class_inheritance` 344) → §3.M2 order; no `extends` → `RefCounted`.
- **Inner classes across files** (`preload(file).Inner` / `GlobalClass.Inner`) → resolved on a CLASS/SCRIPT meta base; chains nest arbitrarily.
- **Cycles** (§3.M2 / D7): staged `EMPTY → PARSED → INHERITANCE_SOLVED → INTERFACE_SOLVED → FULLY_SOLVED`; re-entering a `RESOLVING` node → `Cyclic reference.`; post-resolution fqcn walk → `Cyclic inheritance.`

---

## 7. Risk list (rated; from the corpus-grounded pass)

| # | Risk | Rating | Mitigation |
|---|---|---|---|
| R1 | **salsa memory regression** (RA ~4×'d RAM, #19402; startup #19404) | **HIGH** | §4.3 mitigations applied from M0; per-milestone memory budget check on the reference corpus. |
| R2 | **`resolve_external` signature ripple wider than it looks** (`&dyn Db`+`FileId` through `resolve_*`/`ClassScope::new`/`analyze_file`/all call sites) | **HIGH** | D2: spike the ripple in M0 with `resolve_external` still returning `Unknown` → provably zero-behavior; byte-identical regression guards it. |
| R3 | **M2 user-`extends` + M4 autoloads UNEXERCISED by the reference corpus** (0 each) | **HIGH** | D10: acquire a second corpus before M2 ships; synthetic fixtures meanwhile. Without it those milestones ship unvalidated. |
| R4 | **`extends`/`preload` cycles** | **MEDIUM** | D7: hand-rolled Godot-faithful `RESOLVING` detection for diagnostics; salsa owns invalidation; shallow-preload tolerance. |
| R5 | **Stale invalidation (THE classic failure)** | **Critical** | Let salsa own invalidation (never hand-roll a memo cache); the `item_tree`-excludes-bodies invariant gated by the event-log test; golden fixtures re-run after edits; determinism tests so staleness can't hide behind ordering. |
| R6 | **Durability mis-tiering** silently re-validates the subgraph per keystroke | **MEDIUM** | Assert tiers in a test; engine API HIGH, project config MEDIUM, edited file LOW; the medium-or-higher skip nuance (§4.2) verified against `base-db/change.rs`, not the plan's binary framing. |
| R7 | **`global_script_class_cache.cfg` staleness if trusted** | **MEDIUM** | D8: source is truth, cache is a hint; enforced by absent/wrong-cache fixtures. |
| R8 | **salsa is pre-1.0 / API churn** (`unwind_if_cancelled` → `unwind_if_revision_cancelled` already drifted) | **MEDIUM** | Pin 0.27.x; re-diff the macro surface before locking; keep usage idiomatic. |
| R9 | **Rename corrupts code if wrong** | **HIGH** | M5: `DefId`-equality (no string-match) → no false positives; conservative scope + `RenameError` refusal; adversarial same-name corpus, zero-false-edit bar. "Correct or it refuses." |
| R10 | **guitkx generated `.gd` dual channel** (`preload`-const vs `class_name`-global) | **MEDIUM** | D9: treat generated `.gd` as ordinary source (real `class_name`); registry unifies both onto one `ScriptRefId`; defer `.guitkx`-aware go-to-def. |
| R11 | **wasm-safety regression with salsa** | **MEDIUM** | Native-only loader for `notify`/`walkdir`/threads; wasm32 gate with salsa every PR; single-writer/multi-reader degrades to single-threaded-sync in the browser. |

---

## 8. Corrections to `PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`

The design plan is sound; these are the deltas the implemented code + re-verified sources force. **None changes the architecture — they sharpen it.**

1. **`load()` is NOT statically resolvable (load-bearing).** The plan resolves `load("res://x.gd")` *literal* identically to `preload` (§3.2, and an `ExternalRef::LoadLiteral` variant). **Wrong per Godot 4.4:** `load` is an ordinary runtime call returning an opaque `Resource`/`Object` — not constant, not a meta-type; `load(...).new()` is not statically typed. **Both `load("lit")` and `load(var)` are opaque.** Handle `load` in `infer.rs`'s `Expr::Call` arm (D5), **not** as `resolve_external` variants. Remove `LoadLiteral`; `LoadDynamic` is unnecessary.
2. **The seam's real shape.** The plan sketches `resolve_external(db, from, what)` with six `ExternalRef` variants returning `Ty::Error` for dangling. The **implemented** enum has **four** variants (`ClassName`/`ExtendsPath`/`Preload`/`Autoload`) and the fn is **context-free** today (`resolve.rs:41`). Reconciliation: thread `&dyn Db, FileId` (D2); keep the four variants; **distinguish "unresolved" from "dangling"**: not-yet-loaded / Phase-3-incomplete → `Ty::Unknown` (no diagnostic, per the M-invariant); a path *definitively absent* in a fully-indexed project → `Ty::Error` + diagnostic. The plan's blanket `Error`-on-dangling would risk false positives during partial load.
3. **Durability is medium-or-higher, not binary HIGH/LOW.** The plan frames the skip as "read only HIGH inputs." Corrected: salsa validates in O(1) any query reading **only MEDIUM-or-higher** inputs when a LOW input changed — so MEDIUM `project.godot` benefits too (§4.2).
4. **salsa provenance.** The plan's "salsa 0.27, AnalysisHost/snapshot, ParallelDatabase" mixes new- and old-salsa. New salsa has **no `snapshot()`/`ParallelDatabase`** — clone the `Send` handle. The cancellation poll is **`unwind_if_revision_cancelled`** (renamed, #447). RA reached salsa 0.27 via PR #18964 (not the design-era assumption); RA *is* on upstream salsa, not only `ra-salsa`.
5. **Memory is a first-class risk (NEW).** The plan rates memory "Medium." RA's salsa port ~4×'d RAM (#19402) — promote to **HIGH** and budget from M0 (§4.3, R1).
6. **The autoload `*` flag — guard against the inverted myth (NEW).** Confirmed `*` = singleton/global (`project_settings.cpp`); the popular web claim "`*` = disabled" is **wrong**. Strip `*`, seed globals only for `*` entries.
7. **Cache moved out of `project.godot` in Godot 4.** `class_name` metadata lives in `res://.godot/global_script_class_cache.cfg` (PR #70557); Godot 3 used `[_global_script_classes]` in `project.godot`. The plan's "cache is a hint" stance is right; the location/format is now pinned (§3.M4 / D8).
8. **No fixed-point name-resolution loop (record WHY).** RA's iterative nameres exists **solely** for macro expansion; GDScript has no macros → resolution is **single-pass**. The plan's `DefMap` should not import RA's `ReachedFixedPoint`/retry machinery. Document this so a future maintainer doesn't "restore parity" with RA by mistake.
9. **Milestone re-ordering (process).** The plan's six parallel Workstreams are re-sequenced into M0–M5 by **corpus demand**: M1 (`class_name`) alone is ~85 % of real references, so it ships first after the M0 substrate; `extends`/`preload`/autoload/navigation layer on additively.

---

## 9. Concrete build order (each step ends in a verifiable gate)

1. **M0.1** — add `salsa 0.27`; `FileText`/`ProjectConfig`/`ApiInput` inputs in `gdscript-db`; `#[derive(Update)]` on stored returns. **Gate:** compiles; wasm32 green with salsa.
2. **M0.2** — convert `parse`/`item_tree`/`analyze_file`/semantic fns to `#[salsa::tracked]`; move the VFS map from `AnalysisHost` into `gdscript-db`; `apply_change` → salsa setter; durability tiers. **Gate:** all Phase-2 tests byte-identical.
3. **M0.3** — diagnostics → `#[salsa::accumulator]`; real cancellation into `Cancellable`. **Gate:** `body_edit_does_not_invalidate_globals` + `signature_edit_invalidates_dependents_only` pass (permanent CI gates); memory budget recorded.
4. **M0.4** — the D2 threading spike: `resolve_external(&dyn Db, FileId, …)` everywhere, still returning `Unknown`. **Gate:** zero behavior change (byte-identical regression).
5. **M1** — `global_registry()` + `script_class()` + activate `Ty::ScriptRef` + `resolve_external(ClassName)`. **Gate:** corpus `V.`/`Hooks.`/`DemoBox.` resolve; 149 typed decls typed; cross-file go-to-def; 0 false diagnostics; cache-absent/wrong fixtures.
6. **M2** — `extends` user/path + the two-realm member walk + cycle detection. **Gate:** synthetic inheritance/cycle fixtures + **second corpus** wired.
7. **M3** — `preload` const-alias meta-type; `load` opaque. **Gate:** corpus 24 preloads typed; 6 `load`s produce no false typing.
8. **M4** — `project.godot` `[autoload]` parse (`*` flag) + autoload globals + `is`/`as` user narrowing. **Gate:** second-corpus autoload + narrowing fixtures.
9. **M5** — find-references, rename (correct-or-refuse), workspace symbols. **Gate:** rename-`class_name`-across-files + adversarial zero-false-edit corpus; find-refs complete + correct.

---

## 10. References

- [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) — the design plan this playbook executes + corrects (§8).
- [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) — the single-file seam (`resolve_external → Unknown`), the `Ty::Unknown`/`Ty::ScriptRef` variants, the pure `(api, root) -> FileInference` fn this phase re-parameterizes.
- [`PHASE-1-IMPLEMENTATION-PLAYBOOK.md`](PHASE-1-IMPLEMENTATION-PLAYBOOK.md) — the house format this doc mirrors.
- **Implemented seams (branch `feat/phase-2`):** `crates/gdscript-hir/src/resolve.rs:24-43,109-174`; `infer.rs:208-261,585-592,642-648,831-841,1068-1077,1131-1155`; `ty.rs:42,97-100,189-276`; `item_tree.rs:5-9,25,208-211`; `gdscript-ide/src/lib.rs:33-99,101-227`; `gdscript-db/src/lib.rs:10-16`; `gdscript-base/src/lib.rs:14-17,295-297,311-314`; `gdscript-api/src/lookup.rs:126-200`.
- **Primary sources (re-verified):** salsa 0.27.1 (docs.rs, salsa book, #447); rust-analyzer `base-db`/`hir-def`/`vfs`/`item_tree.rs`/`guide.md`/`architecture.md` + #18964/#19402/#19404; Godot 4.4 `gdscript_analyzer.cpp` (`reduce_identifier` 4319, `reduce_preload` 4647, `make_script_meta_type` 116, `resolve_class_inheritance` 344), `project_settings.cpp` (autoload `*`), `script_language.cpp` (`ScriptServer`), `gdscript_cache.cpp`; Godot PR #70557 + cache bugs #72989/#75684/#77478/#102568; reference corpus `ReactiveUI-Gadot`.
