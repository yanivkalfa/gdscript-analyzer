# PHASE 2 — API + Single-File Semantics (Tier 1) — ★ THE MVP ★

> **Status:** plan. **Tier:** 1 (single-file inference). **Completes:** the MVP.
> **Canonical parents this doc obeys:** [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§1 crate stack, §2 the `AnalysisHost`/`Analysis` API, §5 data model, §7 portability), [`ROADMAP.md`](ROADMAP.md) (Phase 2 deliverable + exit criteria, Tier 1).
> **Primary evidence:** [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) (HIR/binder/checker, gradual typing, the `UNSAFE_*` family), [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) (name resolution, the 48 warnings, the LSP feature set), [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md) (the `extension_api.json` model the type layer consumes), [`GODOT-SYNC.md`](GODOT-SYNC.md) (the gdscript-api data pipeline; produced in Phase 0).

This phase turns "we can parse GDScript" (Phase 1, Tier 0) into "we understand a single GDScript file" (Tier 1). It is the **biggest perceived-quality jump** in the whole roadmap and it is **self-contained**: everything here works on **one `.gd` file plus the bundled engine API**, with **no project graph, no scenes, and no cross-file resolution** (those are Phases 3/4). It is also the phase that lets **guitkx delete its `godotProxy.ts`** — answering embedded-GDScript completion/hover from our napi build instead of a running Godot editor.

---

## Goal & scope (Tier 1, SINGLE-FILE)

### What ships

1. **`gdscript-api` fully wired** — the generated engine model (classes, inheritance chain, methods, properties, signals, enums, constants, singletons, utility functions, ~38 builtin Variant types) loaded from a Phase-0 codegen artifact, **plus** the hand-authored GDScript layer the dump omits (keywords, the 36 annotations, GDScript builtins `preload`/`load`/`range`/`len`/…, the `@GlobalScope`/`@GDScript` pseudo-classes), **plus** the doc-XML hover index (BBCode→Markdown).
2. **`gdscript-hir` single-file semantics** — AST→HIR lowering (`ItemTree` = signatures, `Body` = function bodies), a TypeScript-style **binder** (symbols + scope chain), and a **forward, bottom-up gradual type checker** with `is`/`as`/`!= null` flow narrowing and the `Variant` escape hatch.
3. **Core type/safety diagnostics** — a curated subset of the Godot warning set with **engine-matching message strings**: `UNSAFE_METHOD_ACCESS`, `UNSAFE_PROPERTY_ACCESS`, `UNSAFE_CALL_ARGUMENT`, `NARROWING_CONVERSION`, `INFERENCE_ON_VARIANT`, plus a `TYPE_MISMATCH` assignability error and `INTEGER_DIVISION`.
4. **IDE features on top (`gdscript-ide`)** — hover (inferred type + doc), member completion (filtered by inferred receiver type), signature help (active param), inlay hints (inferred types on `:=` and unannotated params), completion for keywords/annotations/globals/utility-functions, parse+type diagnostics, and **basic code actions** ("add type annotation", "annotate inferred type").
5. **guitkx first-client validation** — the napi build answers the completion + hover questions that `ide-extensions/lsp-server/src/godotProxy.ts` currently asks a running Godot editor, on the same in-memory virtual `.gd` text, with **no editor running**.

### Explicit non-goals (deferred)

| Deferred capability | Phase | Why not here |
|---|---|---|
| Cross-file resolution: `class_name` registry, `preload`/`load`/`extends "path"` edges, autoloads | **3** | Needs a project model + VFS scan; single-file is the unit Phase 3 composes. In Phase 2 these degrade to `unknown`/`Variant` gracefully. |
| salsa incremental query graph + durability | **3** | Phase 2 uses plain maps + whole-file recompute (files are small). Every derived computation is written as a pure `(db, file) -> value` fn so the swap is localized. |
| go-to-definition / find-references / rename / workspace symbols | **3** | All need the cross-file index. (In-file go-to-def is *possible* but is held to Phase 3 to keep the boundary clean.) |
| Scene-aware node-path typing (`$Path`/`%Unique`/`get_node("...")` → concrete `Button`) | **4** | Needs `.tscn` parsing. In Phase 2 `$X`/`get_node(...)` type to **`Node`** (the engine's own static type). |
| The full **48-warning** set + project-settings gating + warnings-as-errors | **6** | Phase 2 ships a **core subset**; gating is *noted* but hard-wired to defaults. |
| Real control-flow-graph narrowing that *beats* Godot on guarded blocks | **6** | Phase 2 does **local, syntactic** narrowing only (the guarded sub-tree of an `is`/`as`/`!= null` test), not a full CFG. |
| Formatter, semantic tokens, folding beyond Phase 1 | 1/5/6 | Out of the Tier-1 semantic scope. |

**Boundary rule (load-bearing for Phase 3):** every place that *would* consult another file — `extends OtherScript`, `preload("res://x.gd")`, a bare `SomeClassName`, an autoload identifier — must funnel through **one** function, `resolve_external(name_or_path) -> Ty` that in Phase 2 returns `Ty::Unknown` (a *distinct* type from `Ty::Variant`; see §2). Phase 3 reimplements only that function. Nothing else in the checker knows whether a type came from this file, the engine API, or another file.

---

## Prerequisites

**From Phase 1 (Tier 0 — parser & syntax MVP):**
- `gdscript-syntax`: the logos lexer + indentation pre-pass + recursive-descent parser producing a lossless `cstree` CST and a typed AST, with error recovery. Tier 1 lowering consumes the **typed AST**.
- `gdscript-base`: `FileId`, `TextSize`/`TextRange`, `LineIndex` (byte↔UTF-16), and the serde POD result structs.
- `gdscript-ide`: the `AnalysisHost`/`Analysis` skeleton (snapshots, `apply_change`, `Cancellable<T>`) already returning parse diagnostics, document symbols, folding, and **by-name (no-type)** completion.
- `gdscript-ffi`: the napi-rs v3 binding (Node `.node` + wasm32) returning Phase-1 POD; Phase 2 only adds new query methods.

**From Phase 0 (ecosystem & the gdscript-api codegen pipeline):**
- The `xtask` codegen that ingests vendored `extension_api.json` (per minor) + doc XML and emits the **`gdscript-api` data artifact** (the engine model + the BBCode→Markdown doc index). [`GODOT-SYNC.md`](GODOT-SYNC.md) owns *how* that artifact is produced and kept fresh; Phase 2 owns *the Rust types that model it and the loader/queries over it*.
- At least one bundled snapshot (the `default_minor`, newest, e.g. `4.7`) present in `vendor/godot/`. Multi-version *selection* logic exists but Phase 2 single-file analysis always uses the default snapshot (project-version detection from `project.godot` is a Phase-3 input).

**Sanity gate before starting:** `cargo check -p gdscript-ide --target wasm32-unknown-unknown` is green on the Phase-1 tree (the portability invariant from [`01`](01-ARCHITECTURE.md) §7).

---

## Workstream 1 — `gdscript-api` data model & loading

`gdscript-api` is the **native half of the type graph** ([`research/09`](research/09-type-system-and-inference.md) §6). It is generated (Phase 0), loaded read-only here, and queried by the checker via the inheritance table. **No `std::fs`** — the artifact is embedded or injected (see "Loading").

### 1.1 The Rust types modeling the engine

```rust
// crates/gdscript-api/src/model.rs  (sketch — illustrative, not final)

/// Interned id for an engine class ("Button", "Node", "Object", "RefCounted").
/// Interning keeps Ty small + Copy and member lookup an index walk, not a string walk.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ClassId(u32);

/// Interned id for one of the ~38 builtin Variant value types (int, String, Vector2, Array, ...).
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct BuiltinId(u32);

/// The whole engine model for ONE Godot minor version. Loaded once, shared (Arc), immutable.
pub struct EngineApi {
    pub version: ApiVersion,                 // header.version_major/minor/patch + precision
    classes:   Vec<ClassData>,               // indexed by ClassId.0
    builtins:  Vec<BuiltinData>,             // indexed by BuiltinId.0
    by_name:   FxHashMap<EcoString, ClassId>,// "Button" -> ClassId  (case-sensitive)
    builtin_by_name: FxHashMap<EcoString, BuiltinId>,
    singletons: FxHashMap<EcoString, ClassId>, // "Input" -> Input, "OS" -> OS, ...
    utilities:  FxHashMap<EcoString, UtilityFn>, // @GlobalScope utility funcs (print, sin, typeof...)
    global_enums: FxHashMap<EcoString, EnumInfo>,// @GlobalScope enums (Side, Key, ...)
    global_consts: FxHashMap<EcoString, ConstInfo>, // PI? no — those are @GDScript; here: UINT8_MAX...
}

pub struct ClassData {
    pub id: ClassId,
    pub name: EcoString,                 // "Button"
    pub base: Option<ClassId>,           // Button -> BaseButton; Object -> None
    pub is_refcounted: bool,
    pub is_instantiable: bool,
    pub api_type: ApiType,               // Core | Editor
    pub methods:    FxHashMap<EcoString, MethodSig>,
    pub properties: FxHashMap<EcoString, PropertyInfo>,
    pub signals:    FxHashMap<EcoString, SignalInfo>,
    pub enums:      FxHashMap<EcoString, EnumInfo>,   // e.g. Control.LayoutPreset
    pub constants:  FxHashMap<EcoString, ConstInfo>,
    pub doc: DocId,                      // index into the doc-XML hover store (lazy)
}

pub struct MethodSig {
    pub name: EcoString,
    pub params: Vec<Param>,              // name, ty (TyRef, the *unresolved* api type ref), default?
    pub return_ty: TyRef,                // TyRef::Void for "no return_value"
    pub is_const: bool,
    pub is_static: bool,
    pub is_vararg: bool,
    pub is_virtual: bool,
    pub doc: DocId,
}

pub struct Param { pub name: EcoString, pub ty: TyRef, pub default: Option<EcoString> }

pub struct PropertyInfo { pub name: EcoString, pub ty: TyRef, pub setter: Option<EcoString>,
                          pub getter: Option<EcoString>, pub enum_of: Option<EcoString>, pub doc: DocId }
pub struct SignalInfo   { pub name: EcoString, pub params: Vec<Param>, pub doc: DocId }
pub struct EnumInfo     { pub name: EcoString, pub is_bitfield: bool,
                          pub values: Vec<(EcoString, i64)>, pub doc: DocId }
pub struct ConstInfo    { pub name: EcoString, pub value: ConstValue, pub doc: DocId }

pub struct BuiltinData {
    pub id: BuiltinId,
    pub name: EcoString,                 // "Vector2", "String", "Array", "Dictionary", ...
    pub members:    FxHashMap<EcoString, TyRef>,     // Vector2.x -> float
    pub methods:    FxHashMap<EcoString, MethodSig>, // String.begins_with(...) -> bool
    pub constants:  FxHashMap<EcoString, ConstInfo>, // Vector2.ZERO
    pub enums:      FxHashMap<EcoString, EnumInfo>,
    pub operators:  Vec<OperatorSig>,    // {op, right: Option<TyRef>, result: TyRef}
    pub indexing_return: Option<TyRef>,  // Array/Dictionary element/value access result
    pub doc: DocId,
}

/// A *reference* to a type as it appears in the API JSON, BEFORE we resolve it to a Ty.
/// (e.g. "int", "Vector2", "Button", "TypedArray::Node", "enum::Control.LayoutPreset", "void").
/// `EngineApi::resolve(&self, TyRef) -> Ty` turns it into the checker's Ty (§2.1).
pub enum TyRef { Void, Builtin(BuiltinId), Class(ClassId),
                 TypedArray(Box<TyRef>), TypedDict(Box<TyRef>, Box<TyRef>),
                 Enum(EcoString /*qualified*/), Variant }

pub struct UtilityFn { pub name: EcoString, pub params: Vec<Param>, pub return_ty: TyRef,
                       pub is_vararg: bool, pub doc: DocId } // e.g. round() -> Variant (overloaded)

pub enum ApiType { Core, Editor }
```

**The inheritance chain — the thing that replaces a trait solver** ([`research/09`](research/09-type-system-and-inference.md) §3.2, §6): `ClassData.base` forms a single-parent chain `Button → BaseButton → Control → CanvasItem → Node → Object`. Member lookup is a **walk up `base` links** collecting the first hit — "simple inheritance table lookup." Implemented once:

```rust
impl EngineApi {
    /// Walk the inheritance chain; first class that declares `name` wins. None = not present anywhere.
    pub fn lookup_member(&self, class: ClassId, name: &str) -> Option<MemberRef<'_>> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            let data = &self.classes[c.0 as usize];
            if let Some(m) = data.methods.get(name)    { return Some(MemberRef::Method(m)); }
            if let Some(p) = data.properties.get(name)  { return Some(MemberRef::Property(p)); }
            if let Some(s) = data.signals.get(name)     { return Some(MemberRef::Signal(s)); }
            if let Some(e) = data.constants.get(name)   { return Some(MemberRef::Const(e)); }
            cur = data.base;
        }
        None
    }
    pub fn is_subclass(&self, sub: ClassId, sup: ClassId) -> bool { /* walk base links */ }
    pub fn class_by_name(&self, n: &str) -> Option<ClassId> { self.by_name.get(n).copied() }
}
```

### 1.2 Loading the generated artifact (lazy)

- **Format:** the Phase-0 codegen emits **rkyv** (zero-copy archive) for the engine model + a separate doc store; `serde`/`postcard` is the documented fallback. Per [`01`](01-ARCHITECTURE.md) §5, the artifact is **several MB**; for wasm it is pruned → rkyv → brotli → fetched as a **separate content-hashed asset** (not `include_bytes!`).
- **Native:** the napi binding may `include_bytes!` the default-minor blob (fast, simple) **or** mmap a vendored file — but the *core crate* never reads the filesystem. `EngineApi` is constructed from a `&[u8]` the caller supplies; `gdscript-api` exposes `EngineApi::from_bytes(&[u8]) -> Result<Arc<EngineApi>>`.
- **Lazy:** rkyv lets us keep the archive bytes and deserialize on access; the **doc store is loaded lazily by `DocId`** (hover is the only consumer and it's cold-path). The class/method tables are eagerly indexed into `FxHashMap`s at load because the checker hits them constantly.
- **wasm-safe:** no `Instant::now`, no `std::fs`, no threads in load. CI runs `cargo check -p gdscript-api --target wasm32-unknown-unknown`.

### 1.3 The hand-authored GDScript layer (what the dump omits)

[`research/03`](research/03-godot-api-sync.md) §2.2–2.5: `extension_api.json` is a **ClassDB/Variant dump only**. These tables are hand-authored in `gdscript-api` (checked into the repo, *not* generated), and merged into the model at load:

```rust
// crates/gdscript-api/src/gdscript_layer.rs  (hand-authored, version-aware where needed)

pub struct GdScriptLayer {
    pub keywords:    &'static [Keyword],     // if/elif/else/for/while/match/when/func/var/const/...
    pub annotations: &'static [Annotation],  // the 36 (@export*, @onready, @tool, @rpc, @icon, ...)
    pub builtins:    FxHashMap<&'static str, UtilityFn>, // preload, load, range, len, char, ord,
                                                         // convert, inst_to_dict, assert, Color8, ...
    pub global_pseudo_consts: &'static [ConstInfo], // PI, TAU, INF, NAN  (@GDScript / @GlobalScope)
}

pub struct Annotation { pub name: &'static str,          // "@export_range"
                        pub params: &'static [Param],     // arity for completion + validation
                        pub is_vararg: bool, pub doc: DocId }
pub struct Keyword    { pub text: &'static str, pub kind: KeywordKind, pub doc: Option<DocId> }
```

- **The 36 annotations** come from [`research/04`](research/04-gdscript-semantics-and-features.md) §1.10 (the authoritative arity table) — used by annotation completion (§6) and placement validation. Docs come from `@GDScript.xml`'s `<annotation>` elements (the *only* XML with them).
- **GDScript builtins** (`preload`, `load`, `range`, `len`, …) are the `@GDScript` `<methods>` set ([`research/03`](research/03-godot-api-sync.md) §2.2). Notably:
  - `preload("res://x.gd")` / `load("...")` → in Phase 2 return `Ty::Unknown` via `resolve_external` (Phase 3 resolves to the script type). `load(var)` (non-literal) → `Ty::Variant`.
  - `range(...)` → `Array` (typed `Array[int]` is reasonable but Godot returns untyped `Array`; we return `Array`).
  - `len(x)` → `int`. Some are intentionally `Variant` (overloaded), e.g. nothing here, but `round()`/`abs()` (utility funcs) are `Variant`-returning ([`research/09`](research/09-type-system-and-inference.md) §1.2) — we honor their declared `Variant` return.
- **`@GlobalScope` / `@GDScript` pseudo-classes** are doc-only namespaces; their *content* is spread across `utility_functions`/`global_enums`/`global_constants` (engine half) + this layer (language half). The global scope (§3) merges both.

### 1.4 The doc-XML hover index (BBCode→Markdown)

[`research/03`](research/03-godot-api-sync.md) §3. Hover text comes from doc XML, **not** the JSON. The Phase-0 codegen converts per-symbol BBCode → Markdown and writes a doc store keyed by `DocId`; `gdscript-api` exposes:

```rust
impl EngineApi { pub fn doc(&self, id: DocId) -> Option<&MarkdownDoc>; }
pub struct MarkdownDoc { pub brief: String, pub body: String, pub tutorials: Vec<(String, String)> }
```

The **BBCode→Markdown converter** (modeled on `make_rst.py`'s `format_text_block`): `[b]→**`, `[i]→*`, `[code]→\`code\``, `[codeblocks]/[gdscript]/[csharp]→` fenced ```` ```gdscript ````, cross-refs (`[ClassName]`, `[method X]`, `[member X]`, `[constant X]`, `[signal X]`, `[param x]`, `[enum X]`, `[annotation @x]`) → backticked names (Phase 2: plain text; Phase 3+: links into the symbol index), `[url=…]text[/url]` → Markdown link, `$DOCS_URL` → `https://docs.godotengine.org/en/<version>`. **Strip unhandled tags** so no literal `[...]` reaches hover. This converter runs at **codegen time** (output cached in the store), not per hover — keeping hover warm-path cheap and the converter out of wasm.

---

## Workstream 2 — HIR lowering (`gdscript-hir`)

Mirror rust-analyzer's split ([`research/09`](research/09-type-system-and-inference.md) §3.1): a per-file **`ItemTree`** (signatures, stable across body edits) and per-function **`Body`** (the inference unit). Lowering is a pure function over the typed AST.

### 2.1 The `Ty` representation

The single most important type. Small, `Clone`, mostly `Copy`; `Variant` is the absorbing element; `Unknown` is the *deferred-to-Phase-3* placeholder (distinct from `Variant` and from `Error`).

```rust
// crates/gdscript-hir/src/ty.rs

#[derive(Clone, PartialEq, Eq)]
pub enum Ty {
    /// One of the ~38 builtin value types: int, float, bool, String, StringName, NodePath,
    /// Vector2, Color, RID, Callable, Signal, the Packed*Arrays, Object (as a base), etc.
    Builtin(BuiltinId),
    /// An engine/native class instance, looked up via the inheritance table (Button, Node, Resource).
    Object(ClassId),
    /// A reference to a SCRIPT class by name/path that we cannot resolve in single-file mode.
    /// Phase 2: produced by extends/preload/class_name refs, treated like Unknown for member lookup.
    /// Phase 3: resolved to a real script-class type. Kept distinct so Phase 3 can light it up.
    ScriptRef(ScriptRefId),
    /// Typed containers. Element/value untyped => inner is Variant. No nesting beyond one level
    /// (Godot disallows Array[Array[int]]); inner of a nested annotation is Variant.
    Array(Box<Ty>),                 // Array[T]; plain Array => Array(Variant)
    Dict(Box<Ty>, Box<Ty>),         // Dictionary[K, V]; plain Dictionary => Dict(Variant, Variant)
    /// A named enum *type* (Control.LayoutPreset, or a local `enum State`). Values are int-compatible.
    Enum(EnumRef),
    /// First-class Signal / Callable values (`signal x`, `func`-as-value, `x.connect`).
    Signal(Option<SignalRefId>),
    Callable,
    /// void — return position only.
    Void,
    /// The gradual-typing top type / escape hatch. Member access through it is "unsafe".
    Variant,
    /// "We can't know yet" — an EXTERNAL ref unresolved in single-file mode (Phase 3 fills these).
    /// Behaves like Variant for completion/diagnostics but is NOT reported as INFERENCE_ON_VARIANT
    /// and is the marker Phase 3 keys on. Never user-visible as a hover string ("?"/elided).
    Unknown,
    /// A type error already reported; suppresses cascade diagnostics. Never user-facing.
    Error,
}
```

`Ty` ↔ `gdscript-api::TyRef`: `EngineApi::resolve(TyRef) -> Ty` (Builtin/Class/TypedArray/TypedDict/Enum/Variant/Void). The checker stores `Ty`, never `TyRef`.

### 2.2 `ItemTree` (the file signature)

```rust
// crates/gdscript-hir/src/item_tree.rs
pub struct ItemTree {
    pub class_name: Option<EcoString>,     // `class_name Foo`
    pub extends: ExtendsRef,               // Native(ClassId) | ScriptPath(String) | Implicit(RefCounted)
    pub is_tool: bool, pub is_abstract: bool,
    pub vars:    Vec<VarDecl>,             // script-level var/const (name, declared Ty?, has_init, ...)
    pub consts:  Vec<VarDecl>,
    pub funcs:   Vec<FuncDecl>,            // SIGNATURES only (name, params w/ declared Ty?, return Ty?,
                                           //   is_static, is_virtual-ish, body: BodyId)
    pub signals: Vec<SignalDecl>,
    pub enums:   Vec<EnumDecl>,            // named + unnamed (unnamed inject int consts into class scope)
    pub inner_classes: Vec<InnerClassDecl>,// class Foo: ...  (own nested ItemTree)
    pub annotations: Vec<AnnotationUse>,   // @onready/@export/... attached to the next decl
}

pub struct FuncDecl {
    pub name: EcoString,
    pub params: Vec<ParamDecl>,            // name, declared Ty? (None => Variant param), default?
    pub return_ty: Option<TypeRefSyntax>,  // None => untyped; Some(Void) => -> void
    pub is_static: bool,
    pub body: BodyId,                      // index into the per-file Body arena
    pub range: TextRange,
}
```

Computed by a pure fn: `item_tree(db, file: FileId) -> Arc<ItemTree>`. **No body lowering happens here** — that's the invariant that lets Phase 3 cache signatures across body edits.

### 2.3 `Body` (per-function, the inference unit)

```rust
// crates/gdscript-hir/src/body.rs
pub struct Body {
    pub exprs:    Arena<Expr>,             // id-based; no Box cycles, cheap to walk
    pub stmts:    Arena<Stmt>,
    pub pats:     Arena<Pat>,              // match/for binding patterns
    pub params:   Vec<PatId>,
    pub block:    BlockId,                 // the function's top block
    pub src_map:  BodySourceMap,           // ExprId <-> AST node range, for hover/inlay/diag spans
}

pub enum Expr {
    Literal(LiteralKind),                  // int/float/bool/String/StringName/NodePath/null/Array/Dict
    Path(Name),                            // bare identifier (resolved by the binder)
    Field { receiver: ExprId, field: Name },          // a.b
    Index { receiver: ExprId, index: ExprId },        // a[i]
    Call  { callee: ExprId, args: Vec<ExprId> },      // f(...)  / a.f(...)
    MethodCall { receiver: ExprId, method: Name, args: Vec<ExprId> },
    Binary { op: BinOp, lhs: ExprId, rhs: ExprId },
    Unary  { op: UnOp, operand: ExprId },
    Ternary { then: ExprId, cond: ExprId, else_: ExprId },  // `then if cond else else_`
    Is  { operand: ExprId, ty: TypeRefSyntax, negated: bool },  // is / is not
    As  { operand: ExprId, ty: TypeRefSyntax },                 // as
    Await { operand: ExprId },
    Lambda { params: Vec<PatId>, body: BlockId },
    SelfExpr, SuperExpr,
    Subscript { .. }, Preload { path: Option<EcoString> }, GetNode { path: Option<EcoString> },
}

pub enum Stmt {
    VarDecl { pat: PatId, ty: Option<TypeRefSyntax>, init: Option<ExprId>, inferred: bool /* := */ },
    Assign  { target: ExprId, op: AssignOp, value: ExprId },
    Expr(ExprId), Return(Option<ExprId>), If { .. }, For { .. }, While { .. },
    Match { scrutinee: ExprId, arms: Vec<MatchArm> }, Pass, Break, Continue, Breakpoint, Assert { .. },
}
```

Computed by `body(db, file, func: BodyId) -> Arc<Body>`. The `BodySourceMap` is what every IDE feature uses to translate an `ExprId` back to a byte `TextRange`.

---

## Workstream 3 — Name resolution / binder (single-file)

A TypeScript-style binder ([`research/09`](research/09-type-system-and-inference.md) §3.2): build **symbols + a scope tree**, then resolve identifiers against it. Lookup order ([`research/04`](research/04-gdscript-semantics-and-features.md) §3, [`research/09`](research/09-type-system-and-inference.md) §2.1): **local → class member → inherited → global.**

### 3.1 The scope tree

```rust
// crates/gdscript-hir/src/resolve.rs
pub enum ScopeKind { Script, Function, Block, For, Match, Lambda }

pub struct Scope {
    pub kind: ScopeKind,
    pub parent: Option<ScopeId>,
    pub entries: FxHashMap<EcoString, Symbol>,   // names introduced AT this scope level
}

pub enum Symbol {
    Local { decl: PatId, ty: Cell<Option<Ty>> },     // var/const/param/for-var/match-binding/lambda-cap
    ClassVar { decl: ItemRef }, ClassConst { decl: ItemRef },
    Func { decl: ItemRef }, Signal { decl: ItemRef },
    EnumType { decl: ItemRef }, EnumValue { decl: ItemRef, value: i64 },
    InnerClass { decl: ItemRef },
    SelfTy,                                           // `self`
}

pub struct Resolver<'a> {
    api: &'a EngineApi,
    item_tree: &'a ItemTree,
    scopes: Vec<Scope>,           // arena
    cur: ScopeId,
}
```

### 3.2 Resolution order (the chain walk)

```rust
impl<'a> Resolver<'a> {
    pub fn resolve(&self, name: &str) -> Resolution {
        // 1. LOCAL: walk Block/For/Match/Lambda/Function scopes up to (not including) Script.
        let mut s = Some(self.cur);
        while let Some(id) = s {
            let sc = &self.scopes[id.0];
            if let Some(sym) = sc.entries.get(name) { return Resolution::Local(sym.clone()); }
            if sc.kind == ScopeKind::Script { break; }
            s = sc.parent;
        }
        // 2. CLASS MEMBER: this file's own vars/consts/funcs/signals/enums/inner-classes + `self`.
        if let Some(sym) = self.script_scope().entries.get(name) { return Resolution::Member(sym.clone()); }
        // 3. INHERITED: walk `extends`.
        //    - extends a NATIVE class  -> EngineApi::lookup_member up the inheritance table.
        //    - extends a SCRIPT path / class_name -> resolve_external(...) => Unknown in Phase 2.
        if let Some(m) = self.lookup_inherited(name) { return Resolution::Inherited(m); }
        // 4. GLOBAL: @GlobalScope utility fns, global enums/consts, builtin TYPE names (Vector2,...),
        //    PI/TAU/INF/NAN, GDScript builtins (preload/load/range/len/...), annotation names (@...),
        //    engine singletons (Input/OS/...). class_name globals + autoloads are Phase 3 -> Unknown.
        if let Some(g) = self.resolve_global(name) { return Resolution::Global(g); }
        Resolution::Unresolved
    }
}
```

- **`self`** resolves to the current file's class type (its `extends` base + own members); `self.member` adds *runtime* member access — when `member` isn't statically known on the class, `self.member` is **`Variant`** (potentially-`Variant`), per [`research/09`](research/09-type-system-and-inference.md) §2.1, not a hard error.
- **`super` / `super.method()`** resolves against the `extends` base's member table (native → inheritance walk; script base → `resolve_external` → `Unknown` in Phase 2, so `super.x` types to `Unknown`).
- **Block scopes:** `for x in …` introduces `x` (typed since 4.2 if `for x: T in …`, else element-type of the iterable, else `Variant`); `match` `var name` bindings; lambda params + captures; `if/while` blocks do **not** introduce names but *do* host narrowing (§4).

### 3.3 Graceful degradation of cross-file refs (the Phase-3 seam)

Single-file mode cannot see other files. Every external reference funnels through one fn and yields **`Ty::Unknown`** (never a diagnostic):

```rust
/// THE Phase-3 seam. In Phase 2 this is a stub. Phase 3 replaces ONLY this function.
fn resolve_external(&self, what: ExternalRef) -> Ty {
    match what {
        ExternalRef::ClassName(_)          // bare `Foo` that isn't native/builtin/in-file
        | ExternalRef::ExtendsPath(_)      // extends "res://x.gd"
        | ExternalRef::Preload(_)          // const X = preload("res://x.gd")
        | ExternalRef::Autoload(_)         // a project.godot [autoload] singleton name
        => Ty::Unknown,
    }
}
```

`Unknown` is engineered to **not** degrade the UX: hover elides it (`?`), member completion on it offers *nothing wrong* (falls back to by-name like Tier 0, or empty), and it never triggers `INFERENCE_ON_VARIANT`. This keeps Phase 2 honest (no false claims about types it can't see) while leaving a clean switch for Phase 3.

---

## Workstream 4 — Type inference (the checker)

A **single forward, bottom-up expression walk** returning a `Ty` ([`research/09`](research/09-type-system-and-inference.md) §3.2). No unification variables — there is nothing to solve backward; `Variant` is the universal escape hatch. The checker consumes the binder's resolutions + the API inheritance table.

### 4.1 The walk

```rust
// crates/gdscript-hir/src/infer.rs
pub struct InferCtx<'a> {
    api: &'a EngineApi, body: &'a Body, resolver: &'a Resolver<'a>,
    expr_ty: FxHashMap<ExprId, Ty>,           // memoized result per expr (also feeds hover/inlay)
    diags: Vec<Diagnostic>,                   // accumulated (§5)
    narrow: NarrowMap,                         // flow facts in the current branch (§4.3)
}

impl<'a> InferCtx<'a> {
    fn infer_expr(&mut self, e: ExprId) -> Ty {
        let ty = match &self.body.exprs[e] {
            Expr::Literal(k) => self.lit_ty(k),                      // 45 -> int, 3.14 -> float,
                                                                     // "x" -> String, [] -> Array(Variant)
            Expr::Path(name) => self.infer_ident(name, e),          // scope lookup -> declared/inferred Ty
            Expr::Field { receiver, field } => self.infer_field(*receiver, field, e),
            Expr::MethodCall { receiver, method, args } => self.infer_method_call(*receiver, method, args, e),
            Expr::Call { callee, args } => self.infer_call(*callee, args, e),
            Expr::Binary { op, lhs, rhs } => self.infer_binary(*op, *lhs, *rhs, e),
            Expr::Unary { op, operand } => self.infer_unary(*op, *operand),
            Expr::Ternary { then, cond, else_ } => self.infer_ternary(*then, *cond, *else_),
            Expr::Is { operand, ty, negated } => { self.infer_expr(*operand);
                                                   self.record_is_guard(*operand, ty, *negated); Ty::Builtin(BOOL) }
            Expr::As { operand, ty } => self.infer_as(*operand, ty),
            Expr::Await { operand } => self.infer_await(*operand),
            Expr::SelfExpr => self.self_ty(),
            Expr::GetNode { .. } => Ty::Object(self.api.class_by_name("Node").unwrap()), // Phase 4 -> concrete
            Expr::Preload { .. } => self.resolve_external(ExternalRef::Preload(..)),       // Phase 3 -> script
            Expr::Index { receiver, index } => self.infer_index(*receiver, *index),
            Expr::Lambda { .. } => Ty::Callable,
            _ => Ty::Variant,
        };
        self.expr_ty.insert(e, ty.clone());
        ty
    }
}
```

### 4.2 The rules (representative, all single-file)

A representative member-access rule, showing where `UNSAFE_PROPERTY_ACCESS` and `Variant`-absorption fall out:

```rust
fn infer_field(&mut self, recv: ExprId, field: &str, e: ExprId) -> Ty {
    let recv_ty = self.narrowed_ty(recv);          // apply flow facts first (§4.3)
    match recv_ty {
        Ty::Object(class) => match self.api.lookup_member(class, field) {
            Some(MemberRef::Property(p)) => self.api.resolve(p.ty.clone()),
            Some(MemberRef::Signal(_))   => Ty::Signal(None),
            Some(MemberRef::Const(c))    => c.value.ty(),
            Some(MemberRef::Method(_))   => Ty::Callable,       // method-as-value
            None => {
                // Not on the inferred type, but a SUBTYPE might have it -> Godot's UNSAFE_PROPERTY_ACCESS.
                self.warn(WarningCode::UnsafePropertyAccess, e, format!(
                    "The property \"{field}\" is not present on the inferred type \"{}\" \
                     (but may be present on a subtype).", self.display(&recv_ty)));
                Ty::Variant
            }
        },
        Ty::Builtin(b) => self.lookup_builtin_member(b, field, e), // Vector2.x -> float, etc.
        Ty::Variant | Ty::Unknown => Ty::Variant,                  // access through Variant is unchecked
        Ty::Error => Ty::Error,
        _ => { /* enum value access, ScriptRef (Phase3), ... */ Ty::Variant }
    }
}
```

| Construct | Rule (single-file) |
|---|---|
| **Literals** | `int`/`float`/`bool`/`String`/`StringName`(`&"…"`)/`NodePath`(`^"…"`)/`null`; `[…]`→`Array(Variant)`; `{…}`→`Dict(Variant,Variant)`. |
| **`var x: T` (annotated)** | `Ty` = resolve(T). If `init` present, check `is_assignable(init_ty, T)` → `TYPE_MISMATCH` / `NARROWING_CONVERSION`. |
| **`var x := init` (`:=`)** | `Ty` = `infer(init)`. If `init_ty` is `Variant` → **`INFERENCE_ON_VARIANT`** (and `x` becomes `Variant`). If `Unknown` → `x` is `Unknown`, **no** warning. |
| **`var x = init` (untyped)** | `x` is `Variant` (dynamic). (Phase 6 may emit `UNTYPED_DECLARATION`; off here.) |
| **Member access `a.b`** | inheritance-table lookup over the API model (+ in-file members for `self`); see code above. |
| **Method call `a.f(args)`** | `MethodSig` from the member set; return = resolve(`return_ty`); check each arg vs param (`UNSAFE_CALL_ARGUMENT` when a supertype is passed where a subtype is required; `TYPE_MISMATCH` on incompatible). Vararg → only fixed params checked. Method not found on a known class → `UNSAFE_METHOD_ACCESS`. |
| **Operators** | builtin operator table: `int/int`→`int` + **`INTEGER_DIVISION`**; `float`→`int` context → **`NARROWING_CONVERSION`**; comparisons→`bool`; `Vector2+Vector2`→`Vector2`; mismatched/unknown operands→`Variant`. |
| **Ternary `a if c else b`** | result = common type of `a`,`b` (identical → that; numeric widen → wider; else → `Variant`). (`INCOMPATIBLE_TERNARY` is Phase 6.) |
| **`x as T`** | `Ty` = resolve(T). Casting **from `Variant`** → also `UNSAFE_CAST` (Phase 2 may emit; minimum: result type only). Makes the line "safe". |
| **`x is T`** | `bool`; records a narrowing fact for the guarded branch (§4.3). |
| **`await sig`** | if operand is `Ty::Signal` → result is the signal's first param type (or `Variant`); if a coroutine call → its return; else `Variant`. |
| **Container element types** | `Array[T]` indexing/`for`-var → `T`; `Dictionary[K,V]` `[]`/`for`-var → `V`; untyped → `Variant`. |
| **Enum values** | named `enum State {…}` → `State.IDLE` is `Ty::Enum(State)` (int-compatible); unnamed enum constants inject `int`s into class scope. |
| **`self` / bare member** | `self` = this file's class type; unknown `self.x` → `Variant` (not an error). |
| **`get_node` / `$` / `%`** | `Node` (Phase 2). Phase 4 resolves to the concrete node class. |
| **`preload` / bare class_name / autoload** | `resolve_external(...)` → `Unknown` (Phase 3). |

### 4.3 The `Variant` escape hatch & the "safe vs unsafe line"

- **Variant absorption** ([`research/09`](research/09-type-system-and-inference.md) §1.5): any operation on a `Variant` (or `Unknown`) receiver whose member isn't statically provable yields `Variant` and, for `Object` receivers with a *missing* member, an `UNSAFE_*` diagnostic. "Most 'I can't help you' cases collapse to *the expression is `Variant`*."
- **Safe vs unsafe line** ([`research/09`](research/09-type-system-and-inference.md) §1.7, [`research/04`](research/04-gdscript-semantics-and-features.md) §2.1): a line is **unsafe** iff its inferred receiver/argument type is `Variant` where a concrete type was needed (i.e. it would emit an `UNSAFE_*`). We compute a per-line "safe" bit as a by-product of the `UNSAFE_*` checks; the LSP can later surface it (Godot's green-line feature). Phase 2 only needs the bit + the diagnostics; the editor rendering is a client concern.

### 4.4 Flow narrowing (local, syntactic — full CFG is Phase 6)

```rust
// Narrowing facts scoped to a branch. Phase 2: syntactic sub-tree only (the `if`'s then/else block).
fn record_is_guard(&mut self, operand: ExprId, ty: &TypeRefSyntax, negated: bool) {
    if let Some(place) = self.as_place(operand) {           // a bare local/member path
        let t = self.resolve_type_ref(ty);
        // then-branch sees `place: t`; else-branch sees the un-narrowed type (Phase 6 does negation).
        self.narrow.push_then(place, if negated { /* widen */ } else { t });
    }
}
```

Phase 2 narrows on: `if x is T:` (then-branch → `x: T`), `var t := x as T` (idiomatic), and `if x != null:` (then-branch drops a `null`-ish nuance — minimal). This is **local** (the lexical guarded block), not a real CFG; that's the explicit Phase-6 upgrade that lets us *beat* Godot's own weak narrowing ([`research/09`](research/09-type-system-and-inference.md) §1.6).

### 4.5 Assignability (one routine)

```rust
fn is_assignable(&self, from: &Ty, to: &Ty) -> Assign {
    use Ty::*;
    match (from, to) {
        (_, Variant) | (Variant, _) => Assign::Ok,            // gradual: Variant <-> anything (may flag UNSAFE_*)
        (Unknown, _) | (_, Unknown) | (Error, _) | (_, Error) => Assign::Ok, // don't cascade
        (a, b) if a == b => Assign::Ok,
        (Builtin(INT), Builtin(FLOAT)) => Assign::Ok,         // int -> float widening
        (Builtin(FLOAT), Builtin(INT)) => Assign::Narrowing,  // -> NARROWING_CONVERSION
        (Object(sub), Object(sup)) => if self.api.is_subclass(*sub, *sup) { Assign::Ok }
                                       else { Assign::No },
        (Array(a), Array(b)) => self.is_assignable(a, b).into(), // covariant per Godot runtime rules
        (Enum(_), Builtin(INT)) => Assign::Ok,                // enum value is int-compatible
        (Builtin(INT), Enum(_)) => Assign::IntAsEnum,         // -> INT_AS_ENUM_WITHOUT_CAST (Phase 6)
        _ => Assign::No,                                       // -> TYPE_MISMATCH
    }
}
```

`infer(db, file, body) -> Arc<InferenceResult>` where `InferenceResult { expr_ty, diags, narrow_at, safe_lines }`. This is the pure fn that becomes `#[salsa::tracked]` in Phase 3 unchanged.

---

## Workstream 5 — Diagnostics (core warning subset)

Phase 2 emits a **curated subset** with **verbatim engine messages** (matching Godot makes diagnostics feel native — [`research/09`](research/09-type-system-and-inference.md) §1.7 "Design takeaway"). The full **48-warning** set + per-project gating is **Phase 6** ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.2–2.3).

### 5.1 The Diagnostic POD

```rust
// crates/gdscript-base/src/diagnostic.rs  (serde POD; no lsp-types)
#[derive(Serialize, Deserialize, Clone)]
pub struct Diagnostic {
    pub range: TextRange,            // byte offsets; client converts to UTF-16
    pub severity: Severity,          // Error | Warning | Info | Hint
    pub code: DiagnosticCode,        // stable string code, e.g. "UNSAFE_METHOD_ACCESS"
    pub message: String,             // verbatim Godot wording
    pub fixes: Vec<CodeAction>,      // optional quick-fixes (§6)
    pub source: &'static str,        // "gdscript-analyzer"
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DiagnosticCode(pub &'static str);  // engine symbolic name; key on NAME, never an int
                                              // (severity/enum ints shift across Godot versions — research/04 §2.2)
```

`Analysis::diagnostics(file)` returns `Cancellable<Vec<Diagnostic>>` = parse diagnostics (Phase 1) ∪ the type diagnostics below.

### 5.2 The Phase-2 subset

| Code | Severity (Phase 2) | Engine message (verbatim — research/09 §1.7) | When |
|---|---|---|---|
| `INFERENCE_ON_VARIANT` | Error | `The %s type is being inferred from a Variant value, so it will be typed as Variant.` | `:=` whose RHS infers to `Variant`. |
| `UNSAFE_PROPERTY_ACCESS` | Warning | `The property "%s" is not present on the inferred type "%s" (but may be present on a subtype).` | `a.b` where `b` ∉ members of the known non-Variant type of `a`. |
| `UNSAFE_METHOD_ACCESS` | Warning | `The method "%s()" is not present on the inferred type "%s" (but may be present on a subtype).` | `a.f(...)` where `f` ∉ methods of the known type of `a`. |
| `UNSAFE_CALL_ARGUMENT` | Warning | `The argument %s of the %s "%s()" requires the subtype "%s" but the supertype "%s" was provided.` | arg's static type is a supertype of the required param type. |
| `NARROWING_CONVERSION` | Warning | `Narrowing conversion (float is converted to int and loses precision).` | `float` flows into `int` (assign/param/return). |
| `INTEGER_DIVISION` | Warning | `Integer division. Decimal part will be discarded.` | `int / int`. |
| `TYPE_MISMATCH` (ours) | Error | `Cannot assign a value of type "%s" to a variable of type "%s".` (close to engine's analyzer error) | `is_assignable` == `No` on a declared target/param/return. |
| `UNSAFE_CAST` *(optional, stretch)* | Warning | `Casting "Variant" to "%s" is unsafe.` | `as T` where operand is `Variant`. |

Notes:
- These map to Godot codes; `TYPE_MISMATCH` is our umbrella for hard analyzer errors that Godot reports as compile errors rather than warnings — kept as a single code for Phase 2, refined in Phase 6.
- `%s` placeholders are filled with `display(&Ty)` (the user-facing type name; `Unknown`/`Error`/inner-`Variant` are never substituted into a message — those paths don't emit).

### 5.3 Project-settings gating (noted; full set = Phase 6)

Godot gates each warning via `debug/gdscript/warnings/<lowercase_name>` (Ignore/Warn/Error), a master `enable`, `treat_warnings_as_errors`, and `exclude_addons`/`directory_rules` ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.3). Phase 2 has **no project model**, so:
- Severities are **hard-wired** to the table above (sensible defaults; note that real Godot ships `UNSAFE_*` **off by default** — we surface them **on** by default because the analyzer's value proposition is exactly these, matching the "enforce static typing" workflow).
- The diagnostic plumbing already carries a stable `code`, so Phase 3/6 can map `project.godot` settings → per-code severity (or suppress) **without touching the checker** — the checker always *emits*; a later filter layer *gates*. Inline `@warning_ignore("name")` suppression is likewise a post-filter keyed on `code` (cheap to add; minimal version may land here as a stretch).

---

## Workstream 6 — IDE features on top (`gdscript-ide`)

Each feature is a pure `(db, FilePosition|FileId) -> POD` fn on `Analysis`, built **only** from data this phase produces (item tree, body, inference result, API model, doc store). POD types live in `gdscript-base` (serde, no `lsp-types` — [`01`](01-ARCHITECTURE.md) §2).

### 6.1 Feature → data needed → POD result

| Feature | `Analysis` method | Data needed | POD result |
|---|---|---|---|
| **Hover** | `hover(pos)` | resolve token → `Symbol`/`Ty`; `expr_ty[expr]`; `EngineApi::doc(DocId)` (Markdown) | `Option<HoverResult { ty_label: String, doc: Markdown, range }>` |
| **Member completion** | `completions(pos)` after `recv.` | `expr_ty[recv]` → inheritance-table member set (or builtin members) | `Vec<CompletionItem>` (kind, label, detail=signature, insert) |
| **Signature help** | `signature_help(pos)` inside `f(…|…)` | `MethodSig`/`UtilityFn` of callee + active-arg index (count commas before cursor, respect nesting) | `Option<SignatureHelp { sigs, active_sig, active_param }>` |
| **Inlay hints** | `inlay_hints(file)` | for each `:=`/untyped decl & unannotated param: the inferred `Ty` (`display`) | `Vec<InlayHint { pos, label: ": T", kind: Type }>` |
| **Keyword/annotation/global completion** | `completions(pos)` (no `.`) | keyword table; annotation table (after `@`); globals (utility fns, builtin types, singletons, `PI`/…); in-scope locals/members | `Vec<CompletionItem>` |
| **Parse + type diagnostics** | `diagnostics(file)` | Phase-1 parse diags ∪ §5 type diags | `Vec<Diagnostic>` |
| **Code actions** | `code_actions(range)` | a decl under range + its inferred `Ty` | `Vec<CodeAction { title, edits: SourceChange }>` |

### 6.2 Notes per feature

- **Hover** — `ty_label` is `display(&Ty)` (e.g. `Button`, `Array[int]`, `Variant`; `Unknown`→elide). `doc` is the cached Markdown for the resolved symbol's `DocId` (engine member, builtin, annotation, keyword). For a `var x := …` hover, show the **inferred** type — this is the MVP exit demo: *hover on `var x := get_node(...)` shows the inferred type* (Phase 2: `Node`; Phase 4 sharpens it).
- **Member completion** — the headline. After `button.`, infer `button: Button`, walk the inheritance table (`Button → BaseButton → Control → CanvasItem → Node → Object`), and list **all** methods/properties/signals/constants up the chain (the MVP exit demo). Receiver `Variant`/`Unknown` → fall back to Tier-0 by-name completion (offer all members, best-effort) so the experience never regresses below Phase 1. Builtin receivers (`"abc".`) list `String` methods.
- **Signature help** — active-param tracking counts top-level commas between the open `(` and the cursor, skipping nested `()[]{}`. Varargs keep the last param "active" past its count.
- **Inlay hints** — *the* differentiator Godot's LSP lacks ([`research/04`](research/04-gdscript-semantics-and-features.md) §5.1). Emit a `: T` hint after every `:=` decl and every unannotated param/`for`-var whose `Ty` is **not** `Variant`/`Unknown` (hinting `: Variant` everywhere is noise; suppress it). Also param-name hints at call sites (stretch).
- **Annotation completion** — after `@`, list the 36 annotations from the hand-authored table with their signatures + doc; validate placement is Phase 6.
- **Code actions (basic)**:
  - **"Add type annotation"** — on `var x = expr` (untyped), insert `: <inferred>` → `var x: T = expr` (only when inferred `Ty` is concrete).
  - **"Annotate inferred type"** — on `var x := expr`, rewrite to explicit `var x: T = expr` (turns implicit inference into an explicit annotation).
  - Both produce a `SourceChange` (a list of byte-range text edits) — the same POD `code_actions`/`rename` return. (Phase 6 adds the diagnostic-attached quick-fixes like "prefix `_`", "add `@onready`".)

---

## Workstream 7 — guitkx first-client validation

This is the MVP's acceptance proof: **the napi build answers what `godotProxy.ts` asks a running editor.** ([`ROADMAP`](ROADMAP.md) Phase-2 exit; the full Volar-style source-map adapter + proxy deletion is **Phase 5** — Phase 2 only proves the single-file API *can* answer.)

### 7.1 What guitkx does today (the path we replace)

The guitkx Node LSP lives at `C:\Yanivs\GameDev\ReactiveUI\ReactiveUI-Gadot\ide-extensions\lsp-server` (TypeScript). For embedded GDScript inside `.guitkx` markup it already:
1. builds an **in-memory virtual `.gd`** (`src/virtualDoc.ts`) from the markup's embedded `{expr}` / hook code, with a **length-preserving source map** (`src/sourceMap.ts`);
2. forwards **completion/hover** for a position in that virtual `.gd` to **Godot's built-in LSP over raw TCP port 6005** via `src/godotProxy.ts` (the `GodotProxy` class: `ensureConnected`→`initialize`→`didOpen` the virtual doc→`completion`/`hover`), **degrading to `null` when no editor is running**;
3. separately ships a **static `classdb/godot-control.json`** dump (`src/classdb.ts`) for `Control` property/signal completion *because the proxy needs a live editor and the dump doesn't*.

The pain points Phase 2 removes: **requires a running Godot editor** on port 6005; quality is **capped by the engine's gradual inference**; the **static `classdb.ts` dump is a parallel, hand-maintained data path**.

### 7.2 The swap (Phase 2 scope)

Add a thin adapter that, given the **same virtual `.gd` text + byte offset** the proxy uses, calls the napi package instead of the socket:

```ts
// ide-extensions/lsp-server/src/analyzerProxy.ts  (Phase 2 — replaces godotProxy.ts's data answers)
import { AnalysisHandle } from "@gdscript-analyzer/core";       // napi-rs v3 .node addon
const h = new AnalysisHandle();                                  // holds the Rust AnalysisHost
h.applyChange({ open: { uri, text: virtualGd } });              // push the virtual .gd text (no fs)
const items = h.completions(uri, byteOffset);                   // member/keyword/global completion
const hover = h.hover(uri, byteOffset);                         // inferred type + doc
```

- **Inputs match** the existing path exactly (a virtual `.gd` string + an offset from the source map), so guitkx's `virtualDoc.ts`/`sourceMap.ts` are **unchanged**; only the *answering backend* changes from `GodotProxy` → `AnalysisHandle`.
- **`classdb.ts` becomes redundant** — `Control`/`Button`/etc. members now come from the analyzer's engine model (the same `extension_api.json` data, but in-process and inheritance-flattened). Phase 2 can leave `classdb.ts` in place as a fallback and prove the analyzer answers the same set; Phase 5 deletes both it and `godotProxy.ts`.
- **No editor required** — the napi build is self-contained; CI and headless dev work.

### 7.3 Acceptance test (against the guitkx repo)

A reproducible smoke test mirroring the existing `scripts/live-full.js` (which proves the proxy path), but pointed at the analyzer:

1. Take a real `.guitkx` fixture (e.g. `examples/guitkx/Counter.guitkx`) → `virtualDoc.ts` → virtual `.gd` + source map.
2. For a `{V.<caret>}` position: `AnalysisHandle.completions` returns the `V` members (parity with the 64-member `V.*` set the proxy returns), and a `button.<caret>` position returns `Button`/`Control`/`Node` members.
3. For a hover on a typed embedded local (`var x := get_node(...)`): `AnalysisHandle.hover` returns a type label + doc, with **no Godot editor running** (the proxy path returns `null` here).
4. **Pass criteria:** the analyzer answers ≥ what the proxy answered for member completion + hover on the embedded GDScript, **without** port 6005, within the perf budget (§ Testing). This retires the "requires a live editor" dependency — the MVP's guitkx-facing promise.

---

## Testing strategy

1. **Inference golden cases** (`fixtures/infer/*.gd` + `*.expected`): source annotated with expected `Ty` per `:=`/decl and expected diagnostics (code + range + message). Harness runs `infer(...)`, snapshots `display(expr_ty)` + diagnostics, compares. Cover: `:=` from literal/method/member/`as`; `Variant` absorption + `INFERENCE_ON_VARIANT`; `int/int` `INTEGER_DIVISION`; `float→int` `NARROWING_CONVERSION`; `a.missing` `UNSAFE_PROPERTY_ACCESS`; `a.missing()` `UNSAFE_METHOD_ACCESS`; supertype arg `UNSAFE_CALL_ARGUMENT`; `is`/`as` narrowing; typed `Array[int]`/`Dictionary[String,int]` element types; enum values; inheritance-chain member lookup (`Button` props resolve through `Control`/`Node`); `extends "res://x.gd"`/`preload`/bare class_name → `Unknown` with **no** spurious diagnostics.
2. **Completion/hover snapshot tests**: `(source, byte offset) → sorted CompletionItem labels` / `HoverResult`. Key asserts mirroring the MVP exit: after `button.` the set ⊇ `{text, pressed, get_parent, queue_free, …}` (Button∪Control∪Node); annotation completion after `@` ⊇ the 36; keyword/global completion includes `preload`/`range`/`PI`/`Vector2`; hover on `var x := get_node(...)` shows `Node`.
3. **guitkx integration smoke test** (§7.3): run against the real guitkx repo's `virtualDoc`/`sourceMap` output via the napi package; assert completion/hover parity **without** a running editor. Gated behind the guitkx repo being present (skip if absent).
4. **Perf benchmark** (`criterion`): single-file analyze (parse→item_tree→infer→diagnostics) **cold < 50 ms**, warm (cached parse/api) **< 5 ms** on a representative ~300-line `.gd`. The api model loads once and is shared (`Arc`), excluded from per-file timing. Track member-completion latency (`button.` → list) as a separate < 5 ms warm metric.
5. **wasm32 check (CI gate)**: `cargo check -p gdscript-api -p gdscript-hir -p gdscript-ide --target wasm32-unknown-unknown` green (no `std::fs`/`Instant`/threads leaked into the type layer). A tiny wasm smoke (load pruned api blob from bytes, analyze a snippet) confirms the data-shipping path.
6. **Differential sanity (optional)**: for a corpus of typed `.gd` files, compare our `UNSAFE_*`/`NARROWING`/`INTEGER_DIVISION` emissions against Godot's own (`--check-only`/editor) on a *typed* subset — flag divergences as fixtures, not hard failures (Godot ships `UNSAFE_*` off by default; we normalize gating before comparing).

---

## Exit criteria (= MVP — mirrors ROADMAP Phase 2)

A testable checklist; all must pass on a **single `.gd` file with no project context**:

- [ ] **Member completion after `button.`** lists `Button`/`Control`/`Node` members (inheritance-chain walk), filtered by the inferred receiver type.
- [ ] **Hover on `var x := get_node(...)`** shows the inferred type (`Node` in Phase 2), with engine doc Markdown.
- [ ] **Inlay hints** render inferred `: T` on `:=` decls and unannotated params (suppressed when `Variant`/`Unknown`).
- [ ] **Signature help** shows the active parameter inside a call to an engine method (from `MethodSig`).
- [ ] **Annotation/keyword/global completion** offers the 36 annotations (after `@`), keywords, builtin type names, `PI`/`TAU`/…, and GDScript builtins (`preload`/`range`/`len`).
- [ ] **Diagnostics:** `TYPE_MISMATCH` + the `UNSAFE_*` subset + `NARROWING_CONVERSION` + `INFERENCE_ON_VARIANT` + `INTEGER_DIVISION` fire with **engine-matching messages**.
- [ ] **Code actions:** "add type annotation" / "annotate inferred type" produce correct `SourceChange`s.
- [ ] **Cross-file refs degrade cleanly:** `extends "res://x.gd"`, `preload(...)`, bare `ClassName`, autoload names → `Unknown` with **no false diagnostics**, member completion not worse than Tier 0.
- [ ] **guitkx validation:** the napi build answers embedded-GDScript completion + hover for a real `.guitkx` virtual `.gd`, **with no Godot editor running**, ≥ what `godotProxy.ts` returned (§7.3).
- [ ] **Perf:** cold single-file < 50 ms, warm < 5 ms.
- [ ] **wasm32 CI** for `gdscript-api`/`gdscript-hir`/`gdscript-ide` is green.

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| **Inference corner cases** (overloaded `Variant`-returning utilities like `round()`/`abs()`; ternary type joins; typed-container covariance; `await` on signal vs coroutine) | Honor the API's declared `Variant` returns verbatim; keep joins conservative (`Variant` when unsure — never *wrong*); cover each in golden fixtures; ternary/`INCOMPATIBLE_TERNARY` precision deferred to Phase 6. |
| **API-model size in wasm** (the dump is several MB) | Prune to needed fields → rkyv (zero-copy) → brotli → ship as a **separate content-hashed asset**, fetched (not `include_bytes!`), per [`01`](01-ARCHITECTURE.md) §5; doc store loaded lazily by `DocId`. Bench the wasm bundle in CI. |
| **Single-file boundary leaking into the checker** (would make Phase 3 a rewrite) | **One** `resolve_external` seam returns `Ty::Unknown`; nothing else knows about cross-file. `Unknown` is a distinct `Ty` variant Phase 3 lights up. Every derived computation is a pure `(db, file)->value` fn (salsa-ready). A test asserts no checker code branches on "is this from another file" except via the seam. |
| **`Variant`/`Unknown` over-spreading kills completion quality** (everything becomes `Variant` → completion offers nothing useful) | (a) Keep `Unknown` ≠ `Variant` so unresolved-external never *reports* as Variant and never triggers `INFERENCE_ON_VARIANT`; (b) on a `Variant`/`Unknown` receiver, **fall back to Tier-0 by-name completion** (offer all members) so it never regresses below Phase 1; (c) suppress `: Variant` inlay hints (noise); (d) golden tests assert concrete types survive through member/method chains where they should. |
| **Matching Godot's exact warning semantics** (wording, when `UNSAFE_*` fires "but may be present on a subtype") | Use the **verbatim** strings from [`research/09`](research/09-type-system-and-inference.md) §1.7; key diagnostics on the symbolic **name**, never the engine's shifting int; optional differential check against a typed corpus (§Testing #6). |
| **Doc XML / BBCode conversion edge cases** leaking `[tags]` into hover | Convert at **codegen time** (Phase 0), cache Markdown in the doc store; converter strips unhandled tags; hover-store fixtures assert no residual `[...]`. |
| **napi/wasm boundary cost** (returning big results per keystroke) | Return only the feature POD (never an AST) across the boundary, by-copy serde; the `AnalysisHandle` keeps the host alive across edits (no re-load per call), per [`01`](01-ARCHITECTURE.md) §4. |

---

## References (relative links)

- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — crate stack (§1), `AnalysisHost`/`Analysis` API (§2), salsa-later decision (§3), FFI/WASM (§4), data model (§5), portability rules (§7).
- [`ROADMAP.md`](ROADMAP.md) — Phase 2 deliverable + exit criteria; Tier 0→3 mapping; dependency graph.
- [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) — the market gap; consumers; the v1 bar.
- [`GODOT-SYNC.md`](GODOT-SYNC.md) — the gdscript-api data pipeline (produced/kept fresh in Phase 0).
- [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) — the codegen that emits the gdscript-api artifact this phase loads.
- [`PHASE-1-PARSER-AND-SYNTAX-MVP.md`](PHASE-1-PARSER-AND-SYNTAX-MVP.md) — the CST/AST + `AnalysisHost` skeleton this phase builds on.
- [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) — fills `resolve_external`; adopts salsa; cross-file goto/refs/rename.
- [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) — sharpens `$Path`/`%Unique`/`get_node` from `Node` to the concrete node class.
- [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) — deletes guitkx's `godotProxy.ts`/`classdb.ts`; the Volar source-map adapter; LSP/CLI/playground GA.
- [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) — the full 48-warning set, project-settings gating, real CFG flow narrowing, formatter.
- [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) — **PRIMARY**: HIR/binder/checker, gradual typing, `Variant` absorption, the `UNSAFE_*` family, Tier 1.
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — **PRIMARY**: name resolution, the 36 annotations, the 48 warnings + gating, the LSP feature→data table.
- [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md) — the `extension_api.json` schema, what the dump omits, doc-XML BBCode→Markdown.
