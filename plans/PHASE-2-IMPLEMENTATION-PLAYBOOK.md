# Phase 2 — Implementation Playbook (API model + single-file semantics = the MVP)

> Authoritative build doc for Phase 2, distilled from the Phase-2 research pass (Godot
> type system + `gdscript_analyzer.cpp`, the warning set in `gdscript_warning.cpp`,
> rust-analyzer + Pyright architecture, the `extension_api.json` schema, and the salsa
> decision). Phase 2 = `gdscript-api` (engine model) + `gdscript-hir` (single-file
> semantics). **This is the MVP**: the point where guitkx's `godotProxy.ts` (TCP proxy
> to a running editor) can be replaced for completion + hover.
>
> Companion docs: [`ROADMAP.md`](ROADMAP.md) (sequence/exit), [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md)
> (crate stack), [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md).

---

## 0. The spine — read this first

**Tier 1 = single file + the bundled engine API.** No project graph, no scenes, no
cross-file resolution. The single most load-bearing decision is the **Phase-3 seam**:
everything that *would* need another file funnels through one function

```rust
fn resolve_external(r: ExternalRef) -> Ty   // Phase 2: always Ty::Unknown
// ExternalRef = ClassName | ExtendsPath | Preload | Autoload
```

**`Ty::Unknown` is a distinct type from `Ty::Variant` — this is a hard rule.** `Unknown`
never fires a warning, never produces a wrong type, is elided from hover, never cascades
a diagnostic, and is the marker Phase 3 keys on. This one constant-returning function +
keeping every query a pure `(inputs) -> output` fn (behind an `AnalysisDb` trait, §6) is
what makes the cross-file (Phase 3) and salsa (Phase 3) upgrades *mechanical, not
structural*. It is the biggest enabler in the whole phase; protect it.

**Deferred (do NOT build now):** cross-file resolution → P3; salsa → P3 (§6);
goto-def/refs/rename → P3; scene `$X`/`get_node` → concrete node type → P4; the full
48-warning set + project-settings gating → P6; real control-flow narrowing → P6.

---

## 1. Scope & exit criteria

### 1.1 Features (each a pure `(db, FilePosition|FileId) -> POD` fn on `Analysis`; PODs in `gdscript-base`, serde, never `lsp-types`)

1. **Hover** — `hover(pos) -> Option<HoverResult { ty_label, doc: Markdown, range }>`: inferred `Ty` + engine doc. `Unknown` elided.
2. **Member completion** *(headline)* — `completions(pos)` after `recv.`: the inheritance-table member set filtered by the inferred receiver type. **`Variant`/`Unknown` receiver → fall back to the Tier-0 by-name completion** so it never regresses below Phase 1.
3. **Signature help** — `signature_help(pos)`: active param by counting top-level commas (skip nested `()[]{}`); varargs keep the last param active.
4. **Inlay hints** — `inlay_hints(file)`: `: T` on `:=` decls + unannotated params/`for`-vars. **Suppressed when `Variant`/`Unknown`** — the differentiator the engine LSP lacks.
5. **Keyword / annotation / global completion** — after `@` → the 36 annotations; keywords; builtin type names; `PI`/`TAU`/`INF`/`NAN`; `preload`/`range`/`len`.
6. **Parse + type diagnostics** — `diagnostics(file)` = Phase-1 parse diags ∪ the §5 type diags.
7. **Basic code actions** — "add type annotation" (`var x = e` → `var x: T = e`) and "annotate inferred type" (`var x := e` → `var x: T = e`); each a `SourceChange`.

### 1.2 Exit / MVP bar (all on a single `.gd` file, no project context)

- Member completion after `button.` lists `Button`/`Control`/`Node` members (inheritance walk).
- Hover on `var x := get_node(...)` shows the inferred type (`Node` in P2) + engine doc Markdown.
- Inlay hints render `: T`, suppressed when `Variant`/`Unknown`.
- Signature help shows the active param in an engine-method call.
- Annotation/keyword/global completion offers the 36 annotations, keywords, builtin types, `PI`/…, `preload`/`range`/`len`.
- Diagnostics: `TYPE_MISMATCH` + the `UNSAFE_*` subset + `NARROWING_CONVERSION` + `INFERENCE_ON_VARIANT` + `INTEGER_DIVISION`, **with engine-matching messages** (§5).
- Code actions produce correct `SourceChange`s.
- **Cross-file refs degrade to `Unknown` with zero false diagnostics.**
- **guitkx validation:** the napi build answers embedded-GDScript completion + hover with no editor running, ≥ what the proxy returned.
- **Perf:** cold single-file < 50 ms, warm < 5 ms (criterion, ~300-line `.gd`; the API model is `Arc`-shared and excluded from per-file timing; member-completion latency tracked separately, < 5 ms warm).
- **wasm32 CI green** for `gdscript-api` / `gdscript-hir` / `gdscript-ide`.

### 1.3 Current crate state — exists vs. build (verified)

| Crate | Now | Phase-2 work |
|---|---|---|
| **gdscript-api** | Phase-0 minimal: `generated.rs` = 3 consts (`GODOT_VERSION="4.5.0-stable"`, `CLASS_COUNT=971`, `BUILTIN_CLASS_COUNT=38`); `lib.rs` = only `godot_version()`. | **Build all of it:** `EngineApi`/`ClassData`/`MethodSig`/`TyRef`/`from_bytes`/`lookup_member` + the GDScript layer + doc store + the rkyv codegen pipeline. |
| **gdscript-hir** | Empty compiling stub (one smoke test); deps already correct (`base`,`syntax`,`api`,`db`). | **Build all:** `ty.rs`, `item_tree.rs`, `body.rs`, `resolve.rs`, `infer.rs`. |
| **gdscript-db** | Empty compiling stub. | **Stays a thin stub** — no concrete `db` type in P2 (§6). |
| **gdscript-ide** | Phase-1 real (`AnalysisHost`, VFS = `Arc<FxHashMap<FileId,Arc<str>>>`, `Change`, `Analysis`; 4 Tier-0 features real; `hover`/`goto_definition` stubbed to empty; **`signature_help`/`inlay_hints`/`code_actions` absent**; annotation/keyword tables already hand-authored in `features.rs`). | Add the 3 missing methods; wire `.`-member completion; call into `gdscript_hir`. |
| **gdscript-base** | Tier-0 PODs only. **Missing** `HoverResult`/`SignatureHelp`/`InlayHint`/`CodeAction`/`SourceChange`/`NavTarget`; `Diagnostic` has `code: String` but no `fixes`/`source`; `CompletionItem` has no `detail`. | Add the Phase-2 PODs; evolve `Diagnostic`/`CompletionItem` **additively** (risk §8.2). |

---

## 2. The type model (`gdscript-hir/src/ty.rs`)

GDScript is **gradually typed over one runtime value type, `Variant`**. The engine's
`DataType` carries `kind` and a `type_source ∈ {Undetected, Inferred, AnnotatedExplicit,
AnnotatedInferred}`. The load-bearing predicate is **`is_hard_type() ⟺ type_source >
Inferred`**: a *hard* type is statically enforced (mismatch = error); a *soft*
(`Inferred`) type is best-effort and **downgraded to `Variant` on conflict** rather than
erroring. We replicate this.

```rust
#[derive(Clone, PartialEq, Eq)]            // NOT Copy: Box in Array/Dict. (Re-evaluate interning only if criterion shows clone cost.)
pub enum Ty {
    Builtin(BuiltinId),         // int, float, bool, String, Vector2, … (interned, §4)
    Object(ClassId),            // engine class OR this file's own class / inner class
    ScriptRef(ScriptRefId),     // another .gd by path/class_name — opaque in P2
    Array(Box<Ty>),             // Array[T]; bare Array => Array(Box::new(Ty::Variant))
    Dict(Box<Ty>, Box<Ty>),     // bare => Dict(Variant, Variant)
    Enum(EnumRef),              // an enum value is assignable to int
    Signal(Option<SignalSigId>),
    Callable,
    Void,
    Variant,                    // gradual top / escape hatch (≈ engine VARIANT ≈ Pyright Any)
    Unknown,                    // the Phase-3 seam marker (distinct from Variant!)
    Error,                      // already-reported; suppresses cascade
}

// type_source lives on the binding, not inside Ty (keeps Ty small):
struct TypedBinding { ty: Ty, source: TypeSource }
fn is_hard(s: TypeSource) -> bool { s > TypeSource::Inferred }
```

**The three top-ish types are distinct on purpose:**
- `Variant` — absorbing top. Untyped decls/returns, Variant-typed engine properties. Assigning *from* Variant into a typed slot is allowed-but-unsafe; inferring `:=` *from* Variant fires `INFERENCE_ON_VARIANT` (Error).
- `Unknown` — the seam. `class_name` globals, autoloads, `preload`, script `extends`. **Never** warns, **never** in hover, **never** cascades.
- `Error` — already diagnosed; downstream suppresses further diagnostics.

On a contradicted **soft** inference → downgrade the binding's `Ty` to `Variant`. On a
contradicted **hard** type → emit `TYPE_MISMATCH`.

**Untyped = Variant propagation:** `var x = e` → `Variant`/soft (+ `UNTYPED_DECLARATION`,
off by default); `var x := e` → `infer(e)`/hard, but `INFERENCE_ON_VARIANT` if `infer(e)`
is `Variant` and **`Unknown` (no warning)** if `infer(e)` is `Unknown`; bare `[1,2]` =
`Array(Variant)`, `{}` = `Dict(Variant,Variant)` — **do not over-infer past these**
(match the engine).

**Engine facts the model must encode (all high-confidence):**
- `$Node`/`%Unique`/`get_node()` → **always `Object(Node)`**, never the concrete child; only `as` narrows it.
- `@onready`/`@export` have **no** type effect.
- Typed-container nesting (`Array[Array[int]]`) is **unsupported** — clear nested element types.
- Subscript element typing is a switch: `Array[T][i]→T`, bare `Array[i]→Variant`, `PackedInt32Array[i]→int`, `Vector3[i]→float`, `String[i]→String`, `Dictionary`/`Object`/`Color`/`Transform3D`/`Plane`/`Projection`[i]→`Variant`.
- `for` var: `Array[T]→T`, `for i in 5→int`, `for c in "abc"→String`, `for k in dict→Variant`, bare `Array→Variant`; `for x: T in …` (4.2+) annotates.
- Container builtin methods/operators (`Array.push_back`, `==`) are still untyped → `Variant`, *even on `Array[T]`* (engine limitation, proposal #14129).

---

## 3. Inference & name resolution (`gdscript-hir`)

**Borrow matrix:** take **rust-analyzer's structure** (`ItemTree` → `ScopeMap`/`DefMap` →
arena `Body` → `Resolver` → `InferenceResult`, all pure fns) and **Pyright's semantics**
(gradual `Variant`/`Unknown` top, bidirectional expected-types, flow-scoped `is`/`as`
narrowing, union return inference). **Discard rust-analyzer's Hindley-Milner machinery**
(`InferenceTable`, `TyVid`, numeric fallback, `at.eq` unification, trait solving) — GDScript
is gradual, not HM: types flow forward from annotations, literals, and the engine API,
never from backward constraint solving.

### 3.1 Pipeline (every arrow is one `pub fn`, all pure)

```
cstree CST (Phase 1)
  │ item_tree(file) -> Arc<ItemTree>      ── pure; NO body lowering  (the P3 cache invariant)
  ▼
ItemTree  (class members, funcs, vars, signals, consts, enums, inner classes, extends)
  │ resolve_scope(&item_tree) -> ScopeMap ── pure (≈ single-class DefMap)
  ▼
ScopeMap  (class bindings + extends chain)
  │ body(file, func) -> Arc<Body>         ── pure (≈ hir-def body lowering)
  ▼
Body { Arena<Expr/Stmt/Pat>, BodySourceMap }   (ExprId ↔ TextRange — every IDE feature maps back through this)
  │ infer(file, body) -> Arc<InferenceResult> ── pure (≈ hir-ty infer, gradual)
  ▼
InferenceResult { expr_ty: FxHashMap<ExprId,Ty>, member_resolutions, diagnostics }
```

**Invariant: `item_tree` never reads bodies** — this is what lets Phase 3 cache
signatures across body edits. `infer(...)` becomes `#[salsa::tracked]` in P3 unchanged.

### 3.2 Binder — TypeScript-style; lookup order **local → class member → inherited → global**

1. **Local** — walk Block/For/Match/Lambda/Function scopes up to (not incl.) Script.
2. **Class member** — this file's vars/consts/funcs/signals/enums/inner-classes + `self`.
3. **Inherited** — walk `extends`: native base → `EngineApi::lookup_member` up the table; script base / `class_name` → `resolve_external(...)` → `Unknown`.
4. **Global** — `@GlobalScope` utility fns, global enums/consts, builtin type names, `PI`/`TAU`/`INF`/`NAN`, GDScript builtins, annotation names, engine singletons (`Input`/`OS`). `class_name` globals + autoloads → `Unknown` (P3).

`self` = the current file's class type; unknown `self.x` → `Variant` (not an error);
`super.x` against the base member table (script base → `Unknown`).

```rust
struct ScopeMap { extends: Option<ExtendsTarget>, members: FxHashMap<Name, MemberDef> }
struct Resolver<'a> {
    scope_map: &'a ScopeMap,
    expr_scopes: &'a ExprScopes,           // body-local chain (≈ RA ExprScopes)
    current: ScopeId,
    narrowing: FxHashMap<NarrowPath, Ty>,  // ← Pyright flow facts (NOT in rust-analyzer)
}
// NarrowPath = an identifier or member/subscript chain (a.b.c)
```

### 3.3 The inference walk — single forward, bottom-up, **bidirectional**; no unification vars

```rust
fn infer_expr(&mut self, e: ExprId, expected: &Expectation) -> Ty
enum Expectation { NoExpectation, HasType(Ty) }     // refilled with DECLARED types (Pyright)
```

- **Synthesis** (no/loose expectation): literals → builtin; `[]` → `Array(Variant)`; `a if c else b` → join of branches; calls → engine-API return type.
- **Checking** (`HasType(t)`): push `t` down (`var2: Array[int] = []` → `Array(int)`); after synthesizing, run `is_assignable(synth, t)`; on failure emit `TYPE_MISMATCH`.

Memoize in `expr_ty: FxHashMap<ExprId, Ty>` (also feeds hover + inlay).

| Construct | Rule |
|---|---|
| `var x: T = e` | expected `HasType(T)`; bind `x: T` (hard) |
| `var x := e` | infer `e`; `:=` from `Variant` → `INFERENCE_ON_VARIANT`; from `Unknown` → `Unknown`, **no warning**; `var x := null` is a **hard error** (guard explicitly) |
| `var x = e` | bind `Variant` (soft); `UNTYPED_DECLARATION` (off) |
| member access | inheritance-table lookup; missing member on **known** type → `UNSAFE_PROPERTY_ACCESS` + `Variant`; `Variant`/`Unknown` receiver → `Variant` (unchecked) |
| call | engine-API return type; untyped fn → `Variant`; missing method on known type → `UNSAFE_METHOD_ACCESS` |
| return | declared `-> T` checked per `return`; else accumulate the union of returned types |
| `int / int` | `int` + `INTEGER_DIVISION` |

### 3.4 `is`/`as` narrowing — LOCAL / syntactic only (real CFG is the P6 upgrade)

Pyright/typing-spec semantics over the **lexical guarded sub-tree, not a real CFG**:
- `if x is T:` — then-branch installs `narrowing[path(x)] = T` (`NP = A ∩ R`). Else: if `A` is a union, subtract `T`; else leave unchanged. **`Variant` IS narrowed by `is`** (≈ Pyright "Any narrowed by isinstance") — this is what makes untyped Godot code usable. The `is` expression is always `bool`.
- `x as T` — optimistic downcast → type the expr `T`; `UNSAFE_CAST` if source is `Variant` (off by default, stretch).
- `if x != null:` — eliminate `null` in the then-branch.
- assignment narrowing: after `x = e`, `narrowing[path(x)] = infer(e)`, bounded by `x`'s declared type.

Narrowing facts are **branch-scoped**: clone the map entering an `if`/`match` arm, discard
on exit, join at merges.

### 3.5 `is_assignable(from, to)` — one routine (engine `check_type_compatibility`, ported). Order matters:

1. `to` is `Variant` → OK.
2. `from` is `Variant` → OK but **unsafe** (gradual escape).
3. `Unknown`/`Error` → never cascade (OK, emit nothing).
4. `to` builtin: same type OK; `int→float` OK (silent); `float→int` OK + `NARROWING_CONVERSION`; enum value → `int` OK. **`Array→Array`: element types must be EXACTLY equal — invariant, no covariance** (engine reality — verify with a fixture, §8).
5. `to` enum: `int` source OK (`INT_AS_ENUM_WITHOUT_CAST`); other enum only if same native type.
6. objects: `null` always OK; else `is_subclass` walk.

### 3.6 Member resolution — `lookup_member(recv: &Ty, name)` (replaces trait solving)

`Object`/`ScriptRef`: search class methods/properties then walk `base` (engine inheritance
+ the script's own `extends`); first declarer wins; record in `member_resolutions`.
`Variant`/`Unknown`: graceful fail → `Variant` + `UNSAFE_*`. `Array`/`Dict`/builtins:
resolve against builtin metadata (`Array.size()→int`).

---

## 4. The engine-API model (`gdscript-api`)

### 4.1 Data model (interning keeps `Ty` small + member lookup an index walk)

```rust
pub struct EngineApi {
    pub version: ApiVersion,
    classes:  Vec<ClassData>,                  // indexed by ClassId.0
    builtins: Vec<BuiltinData>,                // indexed by BuiltinId.0 (~38 Variant types)
    by_name:         FxHashMap<Str, ClassId>,
    builtin_by_name: FxHashMap<Str, BuiltinId>,
    singletons:      FxHashMap<Str, ClassId>,  // "Input" -> Input
    utilities:       FxHashMap<Str, UtilityFn>,// @GlobalScope fns
    global_enums:    FxHashMap<Str, EnumInfo>, // "Error","Key","Variant.Type"
    global_consts:   FxHashMap<Str, ConstInfo>,// EMPTY from JSON in 4.5 — filled by the GDScript layer
}
struct ClassData { name, base: Option<ClassId>, is_refcounted, is_instantiable, api_type: ApiType,
                   methods, properties, signals, enums, constants, doc: DocId }
struct MethodSig { name, params: Vec<Param>, return_ty: TyRef, is_const, is_static, is_vararg, is_virtual, doc: DocId }
struct Param { name, ty: TyRef, default: Option<Str> }
struct PropertyInfo { name, ty: TyRef, setter, getter: Option<Str>, enum_of: Option<Str>, doc: DocId }
struct BuiltinData { name, members, methods, constants, enums,
                     operators: Vec<OperatorSig>, indexing_return: Option<TyRef>, doc: DocId }
struct OperatorSig { op: Str, right: Option<TyRef>, result: TyRef }   // right=None ⇒ unary

/// Unresolved API type-ref parsed from the JSON type-string grammar. resolve(TyRef)->Ty.
enum TyRef { Void, Variant, Builtin(BuiltinId), Class(ClassId),
             TypedArray(Box<TyRef>), TypedDict(Box<TyRef>, Box<TyRef>),
             Enum { qualified: Str, bitfield: bool } }
```

The checker stores `Ty`, never `TyRef`. `Str` = a workspace interned string
(`EcoString`/`SmolStr` — **add to the workspace**).

### 4.2 `extension_api.json` schema gotchas the codegen MUST handle (from the real 4.5 file)

- **`classes` is alphabetical, NOT topologically sorted** — 448 cases where a base appears after its derived. **Resolve `inherits` → `ClassId` in a second pass by name.** Only `Object` has no `inherits`.
- **Return-type asymmetry**: engine classes use `return_value: {type, meta}`; builtin-class + utility methods use flat `return_type: string`. Normalize both → one `TyRef`. No return field ⇒ `void`.
- **Enum-property gotcha**: an enum-typed property reports its storage type (`Node.process_mode` → `"int"`), but its getter's `return_value.type` is `"enum::Node.ProcessMode"`. **Populate `PropertyInfo.enum_of` from the getter.**
- **Type-string grammar** `resolve()` must parse: `enum::Class.Enum` / `enum::GlobalEnum`, `bitfield::`, `typedarray::T`, `typeddictionary::K::V`; bare = builtin / class / `Variant` / `void`.
- `api_type ∈ {core, editor}` — gate editor-only symbols out of runtime completion.
- `global_constants` is **empty** in 4.5 — `PI`/`TAU`/`INF`/`NAN` + `@GlobalScope` consts are the hand-authored GDScript layer.
- `meta ∈ {int8..uint64, float, double, char32}` collapses to `int`/`float` for inference; keep only if hover wants exact width.
- `default_value` is a GDScript-literal **source string** (`"Vector2(0, 0)"`) — display verbatim, do not evaluate.
- Volume: 971 classes / 15,889 methods / 3,983 properties / 486 signals / 716 enums; 38 builtins / 998 methods / 749 operators.

### 4.3 Lookup API

```
class_by_name(&str)->Option<ClassId>     builtin_by_name(&str)->Option<BuiltinId>
lookup_member(class,name)->Option<MemberRef>   // walk base, first declarer wins
is_subclass(sub,sup)->bool                resolve(TyRef)->Ty
members_of(class)->impl Iterator<MemberRef>    // INHERITED members (dedup, nearest wins) — for `node.<TAB>`
virtual_methods(class)                    enum_values(scope,enum)
singleton(&str)  utility(&str)  global_enum(&str)  global_const(&str)
builtin_member/method/operator(...)       property_type(class,name)   // enum_of when set, else ty
doc(DocId)->Option<&MarkdownDoc>
// MemberRef<'a> = borrowing enum { Method/Property/Signal/Const/Enum(&…) } — no clone
```

### 4.4 Hand-authored GDScript layer (the JSON omits this) — checked into the repo, merged at load

`keywords`, the **36 annotations** (arity table), GDScript builtins
(`preload`/`load`/`range`/`len`/`char`/`ord`/…), `@GlobalScope`/`@GDScript` pseudo-consts
(`PI`/`TAU`/`INF`/`NAN`). Decided returns: `range(...)→Array`, `len(x)→int`,
`preload/load(literal)→Unknown` (via the seam), `load(var)→Variant`.

### 4.5 Generation: **runtime-parse a binary blob, NOT codegen-to-Rust**

Codegen-to-Rust for ~16k methods + ~4k properties would cripple `rustc` and bloat the
binary. Instead:
1. **Two-stage lower**: permissive `Raw*` serde structs mirroring the JSON → normalized owned `EngineApi` (normalize the §4.2 asymmetries; resolve `inherits` 2nd pass).
2. **Emit a binary blob** to `crates/gdscript-api/src/<minor>.bin` (or `assets/`). `generated.rs` shrinks to `include_bytes!` (native) + `EngineApi::from_bytes(&[u8]) -> Result<Arc<EngineApi>>`.
3. **Format: rkyv (zero-copy) primary**, postcard/serde fallback. **Add `rkyv` + an interned-string crate to `[workspace.dependencies]`** (not present yet). Class/method tables eagerly indexed; doc store lazy by `DocId`.
4. **wasm vs native**: the core crate never uses `std::fs`. **Native** (napi): `include_bytes!` the default-minor blob (simplest first slice; mmap only if size bites). **wasm**: ship the blob as a separate **rkyv → brotli, content-hashed asset** fetched at runtime — never `include_bytes!`. The BBCode→Markdown converter + JSON parser run at **codegen time only** — keep them out of wasm.
5. **Validation gate**: `codegen-api` asserts golden symbols at build time (`Node.add_child` resolves, `Input` is a singleton, `Vector2 + Vector2 → Vector2`) so a bad regen fails loudly.

### 4.6 Doc-XML hover (status: NOT vendored — `vendor/godot/4.5-stable/doc/classes/` is empty)

Phase-2 hover **requires fetching** the XML at tag `4.5-stable`: engine prose
`doc/classes/*.xml`; globals `doc/classes/@GlobalScope.xml`; **annotations + GDScript
builtins (exist nowhere else): `modules/gdscript/doc_classes/@GDScript.xml`**. `xtask`
fetches → `codegen-api` runs a **BBCode→Markdown converter at codegen time** (modeled on
Godot's `make_rst.py`): `[b]→**`, `[i]→*`, `[code]→\`…\``, `[codeblocks]/[gdscript]→`
fenced ```gdscript, cross-refs (`[ClassName]`, `[method X]`, `[member X]`, `[constant X]`,
`[param x]`, `[enum X]`, `[annotation @x]`) → backticked text (P2) / links (P3+),
`[url=…]text[/url]` → MD link, `$DOCS_URL → https://docs.godotengine.org/en/4.5`, **strip
unhandled tags** (no literal `[...]` leaks). Output → a **`DocId`-keyed store, loaded
lazily**, separate from the model blob.

**Defer-vs-now:** wire `DocId` fields into the model **from day one**; the doc store may
be empty if the XML fetch slips — hover then returns `None` and degrades to
signatures-only, while inference/completion (which need zero doc XML) ship fully.

---

## 5. The MVP warning set (engine-matching messages)

**Severities are hard-wired and keyed on a stable string `code`** (never the engine's
shifting int). Messages are `vformat` templates filled at the raise site — reproduce both
the template AND the symbol-substitution order verbatim.

| Code | Engine dflt | P2 dflt | Message (`%s` filled at raise) | Trigger |
|---|---|---|---|---|
| `INFERENCE_ON_VARIANT` | ERROR | ERROR | `The %s type is being inferred from a Variant value, so it will be typed as Variant.` | `:=`/inferred param whose initializer is statically `Variant` |
| `TYPE_MISMATCH` (our umbrella) | (hard `push_error`) | ERROR | our message | incompatible **hard** types, no implicit conv |
| `NARROWING_CONVERSION` | WARN | WARN | `Narrowing conversion (float is converted to int and loses precision).` | `float` stored into `int` slot |
| `INTEGER_DIVISION` | WARN | WARN | `Integer division. Decimal part will be discarded.` | binary `/` with both operands `int` |
| `UNSAFE_PROPERTY_ACCESS` | IGNORE | **WARN** | `The property "%s" is not present on the inferred type "%s" (but may be present on a subtype).` | property not statically found on a **known** base |
| `UNSAFE_METHOD_ACCESS` | IGNORE | **WARN** | `The method "%s()" is not present on the inferred type "%s" (but may be present on a subtype).` | method not statically resolved on a **known** base |
| `UNSAFE_CALL_ARGUMENT` | IGNORE | **WARN** | `The argument %s of the %s "%s()" requires the subtype "%s" but the supertype "%s" was provided.` | arg static type is a *supertype* of the param's required subtype |
| `UNSAFE_CAST` | IGNORE | **off** (stretch) | `Casting "Variant" to "%s" is unsafe.` | `as` where operand is `Variant` |

**Deliberate divergence:** the `UNSAFE_*` family is IGNORE-by-default in Godot, but **we
surface them ON** — they are exactly the analyzer's value proposition.
`UNSAFE_CALL_ARGUMENT`/`UNSAFE_VOID_RETURN` need resolved signatures — emit only when the
receiver is a **known** engine/builtin/own-class type, never on `Variant`/`Unknown`.
`UNSAFE_CAST` ships off.

**Bonus syntactic warnings (Tier 1, zero inference — bundle if time allows; NOT in the exit bar):**
`UNUSED_VARIABLE`, `UNUSED_LOCAL_CONSTANT`, `UNUSED_PARAMETER`, `UNUSED_PRIVATE_CLASS_VARIABLE`,
`UNREACHABLE_CODE`, `STANDALONE_EXPRESSION`, `STANDALONE_TERNARY`, `UNASSIGNED_VARIABLE`, `EMPTY_FILE`.

**Gating:** the checker **always emits**, keyed on the stable `code`. A gating post-filter
(project settings `debug/gdscript/warnings/<code>`, `@warning_ignore`) is **deferred** to
P3/P6. Honor inline `@warning_ignore(...)` only if cheap (single-file, parser-local —
stretch). There is **no global `treat_warnings_as_errors`** in current master.

**Defer entirely (need cross-file/project context):** `UNSAFE_VOID_RETURN` (unless callee
same-file/engine), `NATIVE_METHOD_OVERRIDE`, `GET_NODE_DEFAULT_WITHOUT_ONREADY`,
`ONREADY_WITH_EXPORT`, `MISSING_TOOL`, `STATIC_CALLED_ON_INSTANCE`,
`SHADOWED_VARIABLE_BASE_CLASS`, `SHADOWED_GLOBAL_IDENTIFIER`, `CONFUSABLE_*`,
`MISSING_AWAIT`/`REDUNDANT_AWAIT`, `DEPRECATED_KEYWORD`.

---

## 6. Salsa: **defer to Phase 3** + the `AnalysisDb` seam

**Decision: stay pure-functions in Phase 2 (HIGH confidence).** Salsa's payoff is
cross-input incremental reuse + backdating; Phase 2 has one input per query (a single
file's text) and a straight `text → parse → hir → infer` pipeline — when the text
changes, every downstream stage re-runs anyway. The flagship rust-analyzer invariant
("typing in a body never invalidates global derived data") is inherently *cross-file*.
Adopting salsa now eats its API churn (0.27, 2026) during the phase where it buys nothing.

**Keep the seam cheap (do all six):**
1. Every query stays a **free pure function** over borrowed plain data; never reaches into the VFS map.
2. One indirection — an **`AnalysisDb` trait** (`file_text`, `parse`, `item_tree`, `body`, `infer`). P2 impl = the `FxHashMap` + whole-file reparse (optional trivial memo). P3 impl = a salsa DB delegating to the **same pure bodies**. IDE features depend only on the trait.
3. Model `(FileId, Arc<str>)` as the single mutable input now — drops into `#[salsa::input]` later, zero body changes.
4. Keep outputs `Eq` + `Arc`-wrapped (enables backdating later, free now).
5. **Name the fns/types at the future salsa boundaries** (`parse` per file, position-independent `item_tree`, `infer` per body) so promotion to `#[salsa::tracked]` is mechanical.
6. When adopted, pin + isolate salsa in the single `base-db`-equivalent crate.

**`gdscript-db` role:** **do NOT introduce a real `db` type in P2.** Write
`item_tree(file)`, `body(file, func)`, `infer(file, body)` taking the VFS text +
`&EngineApi` (or behind `AnalysisDb`). `gdscript-db` stays a thin stub until the P3 swap.

---

## 7. Work breakdown & sequencing (bottom-up; each layer compiles against a real lower one)

**Sanity gate before starting:** confirm `cargo +stable-x86_64-pc-windows-gnu check
--target wasm32-unknown-unknown` is green for the three crates (build env: WinLibs mingw
on PATH, run from PowerShell — see the project memory).

### Step 0 — Workspace deps + base PODs (`gdscript-base`, workspace `Cargo.toml`)
- Add `rkyv` + an interned-string crate (`EcoString`/`SmolStr`) to `[workspace.dependencies]`.
- Add Phase-2 PODs to `gdscript-base`: `HoverResult`, `SignatureHelp`, `InlayHint`, `CodeAction`, `SourceChange`, `NavTarget`.
- Evolve `Diagnostic` **additively** (`fixes: Vec<CodeAction>`, a `source` field; keep `code` as the stable string), add `detail` to `CompletionItem`. **Keep additive** — don't break `gdscript-ffi`/guitkx (§8.2).

### Step 1 — `gdscript-api` model + codegen (`gdscript-api`, `xtask`) — the critical path
1. `Raw*` serde → normalized `EngineApi`; resolve `inherits` 2nd-pass; normalize return-type asymmetry; populate `enum_of`; parse the `TyRef` type-string grammar.
2. Extend `xtask codegen-api` → emit the **rkyv blob** + golden-symbol validation; shrink `generated.rs` to `include_bytes!` + `from_bytes`.
3. Lookup API (§4.3): `class_by_name`, `lookup_member` (base walk), `is_subclass`, `members_of`, `resolve(TyRef)`, builtin member/operator, globals.
4. Hand-authored GDScript layer (§4.4).
- *Thin slice:* skip the doc store + BBCode converter (hover → signatures-only). *Full:* + doc-XML fetch + converter + lazy `DocId` store (can parallelize).

### Step 2 — `gdscript-hir` type model + item tree + body
1. `ty.rs`: the `Ty` enum (§2) + `is_assignable` (§3.5) + `TypeSource`.
2. `item_tree.rs`: pure `item_tree(file)` (NO body lowering).
3. `resolve.rs`: `ScopeMap` + `Resolver` + `resolve_external -> Unknown`.
4. `body.rs`: `Body` arena + `BodySourceMap` (ExprId↔TextRange); pure `body(file, func)`.

### Step 3 — `gdscript-hir` inference
1. `infer.rs`: the bidirectional walk (§3.3), `expr_ty` memo, `:=`/return inference, branch-scoped `is`/`as` narrowing, member resolution via the engine API, the §5 diagnostics.
- *Thin slice:* literals + `:=` + member access + `is_assignable` (covers `INFERENCE_ON_VARIANT`, `NARROWING_CONVERSION`, `INTEGER_DIVISION`, `TYPE_MISMATCH`). *Full:* + narrowing + `UNSAFE_*` + return-union.

### Step 4 — `gdscript-ide` wiring
1. Define the `AnalysisDb` trait (§6); wire `Analysis` to call `gdscript_hir`.
2. Implement `hover`; add `signature_help`, `inlay_hints`, `code_actions`; extend `completions` with `.`-member completion (fall back to Tier-0 on `Variant`/`Unknown`).
3. Merge §5 type diagnostics into `diagnostics(file)`.

### Step 5 — guitkx validation + perf + wasm CI (`gdscript-ide`/napi, `xtask`)
1. Thin `analyzerProxy.ts` adapter answering the completion+hover questions `godotProxy.ts` asks (same virtual `.gd` + byte offset, no editor); `virtualDoc.ts`/`sourceMap.ts` unchanged. **Gated on the guitkx repo being present (skip if absent).**
2. criterion benches: cold < 50 ms, warm < 5 ms, member-completion < 5 ms warm.
3. Confirm wasm32 CI green for the three crates.

**Critical path:** 0 → 1 → 2 → 3 → 4 → 5 (Step 2 + the doc half of Step 1 parallelize).
**Thinnest end-to-end MVP:** Step 0 (PODs) + Step 1 (model+lookup, no docs) + Step 2 +
Step 3-thin + Step 4 (member completion + hover-type + the 4 type-only diagnostics).
Everything else layers on.

**Validation throughout:** re-run the real-corpus runner
(`cargo run -p gdscript-ide --example corpus -- <ReactiveUI-Godot>`) — Phase 2 must keep
**0 panics** and produce **no false type diagnostics** on that corpus (the `Unknown` seam
makes this achievable).

---

## 8. Open questions, risks, must-verify

**Resolved conflicts (flag for fixture verification):**
- **Array covariance** — design prose said "covariant"; the primary-source read of `check_type_compatibility` says **element types must be exactly equal (invariant)**. **Trust the source: invariant.** Verify: `var a: Array[Node] = [some_button]`.
- **`Ty` `Clone` vs `Copy`** — `Clone`-only (Box in Array/Dict). Defer the container-interning decision to a criterion benchmark.

**Open design questions (recommendations):**
1. `gdscript-db` role → thin stub; use the `AnalysisDb` trait, no concrete `db` (§6).
2. **POD schema change → additive** evolution of `Diagnostic`/`CompletionItem`. **Biggest compatibility unknown** — check the `gdscript-ffi` layer compiles before changing `gdscript-base`.
3. api-model bytes at runtime (native) → `include_bytes!` first; mmap only if size bites. The rkyv artifact format/location is designed in Step 1.
4. `UNSAFE_CAST` + inline `@warning_ignore` → both optional/stretch.
5. ternary/`await`/typed-container corners → conservative (`Variant` when unsure); precision → P6.
6. guitkx repo at `C:\Yanivs\GameDev\ReactiveUI\ReactiveUI-Gadot\ide-extensions\lsp-server`; validation test conditional.
7. in-file goto-def held to P3 (a decided deferral to keep the boundary clean).

**Must verify against Godot 4.5-stable before/while implementing:** array element invariance; the exact `vformat` symbol order per §5 message; doc XML at `4.5-stable` (incl. `modules/gdscript/doc_classes/@GDScript.xml`) fetches cleanly; `$Node`/`get_node` → `Node`; `:= null` is a hard error; the subscript/for-loop element-type switch tables (one fixture each).

**Biggest RISK:** the engine-API generation pipeline (Step 1) — the rkyv artifact +
two-stage normalizer + `TyRef` grammar parser + doc-XML converter don't exist yet and
carry the subtle gotchas (alpha-not-topo sort, return-type asymmetry, enum-property
getter cross-ref). Everything downstream blocks on it. Mitigate: land model + lookup
**without** the doc store first; gate with golden-symbol assertions.

**Biggest ENABLER:** the `resolve_external -> Ty::Unknown` seam. A distinct,
non-cascading, non-diagnosable type means the entire "no cross-file, no false
diagnostics" criterion is satisfied by one function returning one constant — and Phase 3
reimplements only that function, leaving every pure inference body unchanged.
