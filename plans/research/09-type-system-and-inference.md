# 09 — Semantic Layer: Type System, Name Resolution, and Inference

> **Scope.** This note designs the *semantic / type-inference layer* of `gdscript-analyzer` — the part that
> turns a parse tree into real intelligence: name resolution, type inference, and a project-wide symbol/type
> model. It is the hardest layer and the one that decides whether the product is "a smart linter" or "a Roslyn
> for Godot." Built-in API ingestion (`extension_api.json`) is covered by a sibling note; here we focus on how
> the **type layer consumes** it.
>
> **Headline finding.** GDScript inference is *dramatically* simpler than rust-analyzer's — no generics to
> infer, no traits, no trait solver (Chalk), no lifetimes, no higher-ranked types. It is **gradual typing**
> (Variant-by-default) with local inference via `:=`. A working single-file inferencer is plausibly
> **~5–10% of rust-analyzer's type-system code** ([DeepWiki: Type System and Inference](https://deepwiki.com/rust-lang/rust-analyzer/5.3-type-system-and-inference)).
> The real engineering difficulty and the killer differentiator both live in the **Godot-specific** parts:
> the `class_name`/autoload/preload project graph and **scene-aware node-path typing** from `.tscn` files —
> things the built-in LSP does weakly or not at all.

---

## 1. GDScript's type system — precisely

GDScript is **gradually typed**: every value is a `Variant` by default, type annotations are optional, and
typed and untyped code coexist in the same file and project
([Static typing in GDScript, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html)).

### 1.1 Ways to introduce a type

| Form | Example | Meaning |
|---|---|---|
| Annotated var/const | `var health: int` | declared type |
| Inferred var/const | `var life := 4` (the `:=` operator) | type inferred from RHS |
| Function param | `func f(a: float = 0.0)` | param type |
| Return type | `func sum(...) -> float:` | return type; `-> void` = no value |
| `Variant` (explicit) | `var x: Variant` | "any type … increases readability" but otherwise like untyped |

Source: [Static typing in GDScript, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html).

### 1.2 Inference rules (`:=`)

- `:=` infers the static type of the variable from the **static type of the initializer expression**, giving
  "the same type safety as an explicit type declaration with less typing." For constants, "there is no
  difference between `=` and `:=`" — a const's type is set automatically from its literal value.
- **Inference is purely local and forward.** RHS type is computed, then assigned to the LHS. There is **no
  bidirectional/global unification** as in Hindley-Milner — a variable's type does not "flow backward" from
  later uses.
- **Inferring from a `Variant` yields `Variant`.** Warning `INFERENCE_ON_VARIANT`:
  *"The %s type is being inferred from a Variant value, so it will be typed as Variant."*
  This is the single most important rule for an analyzer: an inferencer must compute "is this expression's
  static type known, or is it `Variant`?" at every node. Most "I can't help you" cases collapse to *the
  expression is `Variant`.*
- Some built-ins are intentionally `Variant`-typed because they're overloaded on argument type (e.g. `round()`
  works on float/int/Vector2/Vector3, so GDScript "cannot know the return type" and yields `Variant`)
  ([GDQuest: Type Inference](https://www.gdquest.com/library/glossary/type_inference/)).

Sources: [Static typing in GDScript, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html);
[gdscript_warning.cpp](https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_warning.cpp).

### 1.3 `class_name` / `extends` form types

- `class_name Item` registers `Item` as a **globally usable type** with no `load`/`preload` needed; it can be
  used anywhere as a type annotation. `@icon(...)` can decorate it.
- `extends` sets the base. Valid forms:
  `extends SomeClass`, `extends "somefile.gd"`, `extends "somefile.gd".SomeInnerClass`.
  Default base is `RefCounted` if none is given. Multiple inheritance is not allowed.
- A script can also be referenced as a type via `const Rifle = preload("res://.../rifle.gd")` — `Rifle` then
  names the script type locally.

So **every `.gd` file is a class**, and the analyzer's notion of "a type" is a tagged union:
*(built-in/native class) | (global class_name) | (script-by-path) | (inner class) | (enum) | Variant | builtin
value type (int, String, Vector2, …) | typed Array/Dictionary | void*.

Sources: [GDScript reference, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_basics.html);
[Static typing in GDScript, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html).

### 1.4 Typed collections

- **Typed arrays** (Godot 4.0+): `var scores: Array[int] = [10, 20, 30]`. The element type "applies to `for`
  loop variables, as well as some operators like `[]`, `[...] =` (assignment), and `+`." Since 4.2 the `for`
  loop variable type can be written explicitly. **Type checking is at runtime on write**, not compile time.
- **Typed dictionaries** (Godot 4.4+, March 2025): `var costs: Dictionary[String, int] = {...}`. Value type
  applies to `for` variables and `[]`/`[...] =`.
- **No nesting.** `Array[Array[int]]` and `Dictionary[String, Dictionary[...]]` do **not** compile; only one
  level deep (`Array[Array]`, inner untyped).
- Element-of-literal types cannot be annotated individually.

Sources: [Static typing in GDScript, 4.4](https://docs.godotengine.org/en/4.4/tutorials/scripting/gdscript/static_typing.html);
[Static typing in GDScript, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html).

### 1.5 `Variant` interaction

`Variant` is the top type and the absorbing element of inference: any operation on a `Variant` receiver whose
member isn't statically provable produces an **unsafe access** (see §1.7). Most "fallbacks" in the inferencer
are "the static type here is `Variant`, so member access is unchecked / completion is best-effort by name."

### 1.6 `is` / `as` narrowing and casting

- **`as` cast**: `var player := body as PlayerController`. "The `as` keyword silently casts the variable to
  `null` in case of a type mismatch at runtime, without an error/warning." It is the standard way to make a
  line *safe* (see below). `var the_node := $Char as CharacterBody2D` is the idiomatic node-typing workaround
  ([GDScript LSP limitations discussion](https://www.gdquest.com/library/glossary/type_inference/)).
- **`is` test**: `if body is PlayerController:` (and `is not`). Idiomatic guard-then-assign pattern lets the
  next assignment to a typed local be checked.
- **Flow narrowing is weak in Godot itself.** A long-standing gripe is that `is`/`as` do **not** reliably
  suppress unsafe-access warnings inside the guarded block — see open issues
  [#8530 (proposal)](https://github.com/godotengine/godot-proposals/issues/8530),
  [#93510](https://github.com/godotengine/godot/issues/93510). **This is an opportunity:** an analyzer with
  real control-flow narrowing (à la TypeScript) can do *better* than Godot's own checker here.

### 1.7 Safe vs unsafe lines + the type-safety warnings

Godot colors line numbers green when a line is provably **type-safe** ("a tool to tell you when ambiguous
lines of code are type-safe"); ambiguous lines are unsafe. `($Timer as Timer)` makes the line green. Safe-line
color can be changed or disabled in editor settings.

A separate, finer mechanism is the **GDScript warning system**. The full `Code` enum
([gdscript_warning.h](https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_warning.h))
has 50 entries; the type/inference-relevant ones, with exact messages from
[gdscript_warning.cpp](https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_warning.cpp):

| Warning | Exact message |
|---|---|
| `UNSAFE_PROPERTY_ACCESS` | "The property "%s" is not present on the inferred type "%s" (but may be present on a subtype)." |
| `UNSAFE_METHOD_ACCESS` | "The method "%s()" is not present on the inferred type "%s" (but may be present on a subtype)." |
| `UNSAFE_CAST` | "Casting "Variant" to "%s" is unsafe." |
| `UNSAFE_CALL_ARGUMENT` | "The argument %s of the %s "%s()" requires the subtype "%s" but the supertype "%s" was provided." |
| `UNSAFE_VOID_RETURN` | "The method "%s()" returns "void" but it's trying to return a call to "%s()" that can't be ensured to also be "void"." |
| `NARROWING_CONVERSION` | "Narrowing conversion (float is converted to int and loses precision)." |
| `INFERENCE_ON_VARIANT` | "The %s type is being inferred from a Variant value, so it will be typed as Variant." |
| `INFERRED_DECLARATION` | "%s "%s" has an implicitly inferred static type." |
| `UNTYPED_DECLARATION` | "%s … has no static return type." |
| `INT_AS_ENUM_WITHOUT_CAST` | "Integer used when an enum value is expected. … cast … using the "as" keyword." |
| `INT_AS_ENUM_WITHOUT_MATCH` | (enum/int match mismatch) |
| `INTEGER_DIVISION` | "Integer division. Decimal part will be discarded." |
| `STATIC_CALLED_ON_INSTANCE`, `NATIVE_METHOD_OVERRIDE`, `PROPERTY_USED_AS_FUNCTION`, `FUNCTION_USED_AS_PROPERTY`, `CONSTANT_USED_AS_FUNCTION` | various member-kind/type misuse |
| `GET_NODE_DEFAULT_WITHOUT_ONREADY`, `ONREADY_WITH_EXPORT` | node/onready typing pitfalls |

Configuration: per-project (Project Settings → Debug → GDScript, "Advanced Settings"), per-line
`@warning_ignore("name")`, region `@warning_ignore_start/_restore`, and "turn them into errors." The
`UNSAFE_*` and `UNTYPED_DECLARATION`/`INFERRED_DECLARATION` warnings are **off by default**
([warning system docs](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/warning_system.html);
[Allen Pestaluky: enforcing static typing](https://allenwp.com/blog/2023/10/03/how-to-enforce-static-typing-in-gdscript/)).

> **Design takeaway.** Reproducing the `UNSAFE_*` warning family *is the type-checker's core diagnostic
> output.* Each one is exactly the question "is the receiver's static type known well enough to prove this
> member/argument/return?" — i.e. the inferencer plus a subtype check. Matching Godot's wording verbatim makes
> diagnostics feel native.

---

## 2. Name resolution & scoping

### 2.1 Scope model

Lookup order for an identifier is **local → class member → global**
([GDScript reference, stable](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_basics.html)).
Concretely the resolver needs a **scope tree** with these node kinds:

1. **Global scope** — `@GlobalScope` built-ins (`print`, `load`, `preload`, `range`, `sin`, …), constants
   (`PI`, `TAU`, `INF`, `NAN`), built-in types (`Vector2`, `Color`, `Array`, …), **all `class_name` globals**,
   and **all autoload singleton names**.
2. **Script (class) scope** — the file's `extends` chain (inherited members), its `var`/`const`/`func`,
   `signal`s, named/unnamed `enum`s, inner `class`es, and the implicit `self`.
3. **Function scope** — params + function locals.
4. **Block scopes** — `for` loop variable (typed since 4.2), `match` pattern bindings, `if/while` blocks,
   lambda captures.

`self` refers to the current instance and adds **runtime** member access (subclass members, `_get`/`_set`),
so `self.x` is checked dynamically whereas bare `x` is checked statically — the analyzer should treat
`self.member` access as potentially-`Variant` when the member isn't statically known.

### 2.2 Inner classes, enums, inheritance

- **Inner classes**: `class SomeInnerClass: …`, instantiated `SomeInnerClass.new()`; nested reference
  `Outer.Inner`. They are first-class types usable in annotations.
- **Enums**: *unnamed* (`enum {A, B, C}`) inject `int` constants into class scope; *named*
  (`enum State {IDLE, JUMP = 5}`) create a constant `Dictionary` and **keys require the `State.` prefix** — and
  `State` itself is a usable enum *type* (relevant to `INT_AS_ENUM_WITHOUT_CAST`).
- **Inheritance**: resolve `extends` to a base type and **flatten the member set** of the chain (script bases
  up to a native class, then native classes via `extension_api.json`). `super`/`super.method()` calls the
  parent.

### 2.3 Global symbol index

Two project-wide registries feed global scope:

- **`class_name` registry** — name → script path (see §4.2).
- **Autoload registry** — singleton name → script/scene path, from `project.godot` `[autoload]` (see §4.3).

### 2.4 Node-path typing (`$`, `%`, `get_node`) — the superpower

`$NodePath` ≡ `get_node("NodePath")`, `%Unique` ≡ `get_node("%Unique")`. All **return `Node`** statically —
Godot's own LSP "cannot automatically know what type of node it is," so users must hand-write
`$Char as CharacterBody2D` to get useful completion
([GDQuest type inference](https://www.gdquest.com/library/glossary/type_inference/)). **But the type is
knowable**: parse the `.tscn` scene that owns this script, resolve the node path within the node tree, and read
the node's declared `type="…"` (and any attached script). See §5. This is the single highest-leverage
Godot-specific feature.

---

## 3. Semantic-layer design (adapted from rust-analyzer / TypeScript)

### 3.1 The HIR idea, and why we want it

rust-analyzer separates *syntax* from *semantics* with a **HIR (high-level IR)** built by lowering the AST,
and computes everything **on demand** through **salsa** queries so edits recompute only what changed
([DeepWiki: Semantic Analysis](https://deepwiki.com/rust-lang/rust-analyzer/5-semantic-analysis);
[architecture.md](https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/architecture.md);
[salsa](https://github.com/salsa-rs/salsa)). Key structures we should mirror in spirit:

- **`ItemTree`** — "condenses a single SyntaxTree into a 'summary' data structure stable over modifications to
  function bodies." For us: a per-file summary of declared classes, members, signals, enums, consts,
  `extends`, `class_name`, `preload`s — i.e. the file's *signature*, deliberately excluding function bodies.
- **`DefMap`** — module tree + scopes (name resolution result). For us: the project symbol graph + per-file
  scope tree.
- **`Body`** — per-function expressions/patterns; the unit of type inference.

The crucial invariant: **"typing inside a function's body never invalidates global derived data."** Editing a
function body re-infers *only that body* because function signatures (params/returns) are explicit boundaries.
GDScript has the same property *iff* signatures are annotated — and even unannotated, a body's local inference
rarely escapes the body. This is what makes keystroke-latency analysis feasible.

### 3.2 The inference algorithm — and how much simpler than rust-analyzer

rust-analyzer's `hir-ty` is Hindley-Milner with heavy extensions: an `InferenceContext`/`InferenceTable`
creates **type variables**, **unifies** them, generates **trait obligations**, runs a **trait solver**
(next-gen/Chalk), handles **generic substitution**, **lifetimes/binders**, **coercions/auto-deref**, and
**numeric fallback** ([DeepWiki: Type System and Inference](https://deepwiki.com/rust-lang/rust-analyzer/5.3-type-system-and-inference)).
The DeepWiki analysis is explicit that a gradual language without generics/traits/HRTs would eliminate generic
lowering+substitution, the entire trait solver (method resolution becomes "simple inheritance table lookup"),
binder/lifetime machinery, and numeric fallback — leaving a **single forward expression walk** and concluding
such a language "could likely implement working type inference in ~5–10% of rust-analyzer's type-system
codebase."

**GDScript inference is therefore closer to TypeScript's checker minus generics minus structural types.** The
TypeScript shape we *should* borrow:
- **Binder** builds **symbols + symbol tables (scopes)** and a **control-flow graph** from the AST.
- **Checker** resolves identifiers against those symbol tables and does **type resolution, contextual/forward
  inference, compatibility checks, and flow-based narrowing**
  ([TS compiler architecture](https://zenn.dev/togami2864/articles/5b6c80cf913b7a?locale=en);
  [TS binder](https://basarat.gitbook.io/typescript/overview/binder)).

Concretely, GDScript inference is a **bottom-up, single-pass expression evaluator** returning a `Ty`:
- literals → their builtin type; `[]`/`{}` → `Array`/`Dictionary` (or typed if annotated);
- identifier → scope lookup → declared/inferred `Ty` (or `Variant`);
- `a.b` → look up `b` in `type_of(a)`'s flattened member set (script chain + native API); unknown member on a
  known non-Variant type → `UNSAFE_PROPERTY_ACCESS` *(if the member could exist on a subtype)* or hard error;
- `a.f(args)` → method signature from member set; check args (`UNSAFE_CALL_ARGUMENT`); result = return type;
- `x as T` → `T`; `$Path` / `%Name` / `get_node("…")` → scene-resolved type, else `Node`;
- `preload("p.gd")` → script type for `p.gd`; `ClassName.new()` → instance of that class;
- binary/unary ops → builtin operator table (incl. `NARROWING_CONVERSION`, `INTEGER_DIVISION`).

**No unification variables are strictly required** for MVP because there is nothing to solve backward; `Variant`
is the universal escape hatch. A light **flow-narrowing** pass (TypeScript-style) over `is`/`as`/`!= null`
guards is the one place worth real CFG work, and it lets us *beat* Godot's own narrowing
([#93510](https://github.com/godotengine/godot/issues/93510)).

### 3.3 Subtyping / assignability

Need one routine: `is_assignable(from, to)`. Rules: identity; `Variant` ↔ anything (assignable both ways but
flagged `UNSAFE_*`/`UNSAFE_CAST` as appropriate); script/native subtype via `extends`/inheritance chain;
numeric `int`→`float` widening ok, `float`→`int` is `NARROWING_CONVERSION`; typed-array/dict covariance per
Godot's runtime rules. The inheritance chain comes from the merged **script graph + native API**.

### 3.4 Incrementality

Use a salsa-style query graph (Rust: the `salsa` crate; or a hand-rolled red-green memo cache):
- inputs: file text, `project.godot`, scene files, `extension_api.json`;
- derived queries: `parse(file)` → CST, `item_tree(file)` → file signature, `class_name_map(project)`,
  `autoloads(project)`, `scene_node_tree(scene)`, `resolve(scope)`, `infer(function_body)`.
- Editing a body invalidates only `infer(body)`. Editing a signature/`class_name`/`extends` invalidates
  dependents transitively but bounded by the dependency graph — **not** the whole project
  ([Durable Incrementality](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html);
  [salsa](https://github.com/salsa-rs/salsa)).

---

## 4. Project-wide model

### 4.1 What "project-wide" requires

Resolve `extends`, `preload`/`load`, `class_name`, and autoloads across files into one **symbol/type graph**,
cached and updated incrementally. Cross-file features (go-to-def, find-refs, rename, project diagnostics)
depend on it.

### 4.2 `class_name` registry — read the cache or scan?

Godot stores the registry in **`.godot/global_script_class_cache.cfg`** — "parsed `class_name` entries …
without requiring the project to be loaded"
([proposal #5631](https://github.com/godotengine/godot-proposals/issues/5631);
[forum](https://forum.godotengine.org/t/godot-class-cache-not-regenerating/55373)).

**Recommendation: scan `.gd` files yourself; treat the cache as an optional warm-start hint only.** Reasons:
the cache is frequently **stale or fails to regenerate** (multiple bugs:
[#77478](https://github.com/godotengine/godot/issues/77478),
[#75388](https://github.com/godotengine/godot/issues/75388),
[PR #102636](https://github.com/godotengine/godot/pull/102636)); it isn't present until the project is opened
in the editor; and we already parse every `.gd` to build `ItemTree`s, so extracting `class_name`/`extends`
during that pass is nearly free and always correct. A startup directory walk + file-watcher keeps it live.

### 4.3 Autoloads from `project.godot`

`project.godot` is an INI-like config with an `[autoload]` section: `Name=*res://path.gd` (or `.tscn`). The
leading `*` (autoload-as-singleton flag) must be **trimmed** when parsing
([Singletons/Autoload docs](https://docs.godotengine.org/en/stable/tutorials/scripting/singletons_autoload.html)).
Each entry injects a global name whose type is the script's class (or the scene root's type for a `.tscn`
autoload). Also parse `[application]`/`run/main_scene` etc. for project context.

### 4.4 `preload` / `load` / `extends` edges

`preload("res://x.gd")` and `extends "res://x.gd"` create file→file edges resolved through a `res://`→fs path
mapper (respect `project.godot` location as the `res://` root). `load(...)` with a string literal is resolvable
identically; dynamic `load(var)` is `Variant`. Build a DAG; detect cycles (legal at runtime for `extends`? no —
cyclic inheritance is an error, so report it).

### 4.5 Incremental strategy

Don't re-typecheck the world per keystroke. Layered invalidation: (a) lexer/parser per file; (b) `ItemTree`
per file (signatures only); (c) project graph updated on `class_name`/`extends`/`preload`/autoload changes
only; (d) body inference on demand for open/visible files. This mirrors rust-analyzer's "body edits don't
touch global data" invariant and salsa memoization (§3.4).

---

## 5. Scene (`.tscn`) awareness — the killer feature

### 5.1 The format (what we parse)

`.tscn` is human-readable INI-like text ([TSCN format, stable](https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html)):

```gdscene
[gd_scene load_steps=4 format=3 uid="uid://cecaux1sm7mo0"]

[ext_resource type="Script"      path="res://player.gd"    id="1_abc12"]
[ext_resource type="PackedScene" path="res://enemy.tscn"   id="2_xyz99"]

[node name="Main"   type="Node2D"]
[node name="Panel"  type="Panel"        parent="."]
[node name="VBox"   type="VBoxContainer" parent="Panel"]
[node name="Button" type="Button"       parent="Panel/VBox"]
script = ExtResource("1_abc12")
[node name="Enemy"  parent="."          instance=ExtResource("2_xyz99")]
```

Rules that matter for typing:
- **Root node** has no `parent=` and there is exactly one; its name is *not* part of child `parent` paths.
- A child of root uses `parent="."`; deeper nodes use slash paths `parent="Panel/VBox"`.
- A node's **Godot class** is its `type="…"`; if it's an **instanced sub-scene** it has
  `instance=ExtResource("id")` (and *no* `type`), so its type is the **root type of the referenced
  `.tscn`** (recursive resolution).
- A node's **attached script** is `script = ExtResource("id")` → a `Script` ext_resource path → a `.gd` class.
- `[ext_resource type="Script"/"PackedScene" path= id=]` maps ids to files. Godot 4.x is `format=3`.

### 5.2 Typing a node path

Given `$Panel/VBox/Button` (or `%Button`, or `get_node("Panel/VBox/Button")`) inside `player.gd`:
1. Find the **owning scene(s)**: scenes whose node has `script = ExtResource(<player.gd>)`. (A script can be
   attached to multiple scenes → multiple candidate trees; type as the **common base** or annotate ambiguity.)
2. Build the scene's node tree from `[node]` headings (name + parent + type/instance).
3. Walk the path from the script-owning node; for `%Unique`, find the node flagged unique-in-owner.
4. The result type is the node's `type` (a **native class** → `extension_api.json`), or the **root type of the
   instanced sub-scene**, refined to the **attached script's `class_name`** if the node has one.

This makes `$Panel/VBox/Button` infer to `Button` (then `button.text` → `String`, `button.pressed` signal,
etc.) **with zero user annotations** — exactly what Godot's own tooling can't do
([GDQuest](https://www.gdquest.com/library/glossary/type_inference/);
[LSP refactor proposal #11056](https://github.com/godotengine/godot-proposals/issues/11056)).

### 5.3 Feasibility / worth

**Feasible and high-value.** The format is simple text (an INI-ish tokenizer + a small node-tree builder; no
need to evaluate resource values). The hard parts are *edge cases*, not core: `uid://` indirection, scripts on
multiple scenes, instanced-scene recursion and inherited scenes, and `@onready` timing. Ship a 90% solution
(direct `type=` nodes in the single owning scene) first; sub-scene recursion and `%Unique` next. This is the
feature most likely to make users say "it's smarter than the built-in editor."

---

## 6. Consuming built-in type knowledge (`extension_api.json`)

`extension_api.json` is Godot's machine-readable dump of all core classes/methods/properties/enums/signals,
produced for binding generators ([DeepWiki: GDExtension API](https://deepwiki.com/godotengine/godot/15.1-gdextension-api)).
The **type layer consumes** it as the **native half of the type graph**:

- **Class table**: name → { base class, is_refcounted/instantiable, methods, properties, signals, enums,
  constants }. Build the **inheritance chain** (`Button → BaseButton → Control → CanvasItem → Node → Object`)
  so member lookup walks up the chain. This is the "simple inheritance table lookup" that replaces
  rust-analyzer's trait solver (§3.2).
- **Method signatures**: name → (param types/defaults, return type, qualifiers: const/static/vararg) → drives
  call inference and `UNSAFE_CALL_ARGUMENT`. Watch for `Variant`-typed returns (e.g. overloaded math) → infer
  `Variant`.
- **Property types**: `button.text` → `String`, `node2d.position` → `Vector2`.
- **Enum types**: native enums (e.g. `Control.LayoutPreset`) for `INT_AS_ENUM_WITHOUT_CAST`.
- **Signal signatures**: for `connect`/`emit` arg checking and signal completion.
- **Builtin value types & operators**: `Vector2`, `Color`, etc., and the operator result table
  (`int / int → int` with `INTEGER_DIVISION`, `float → int` `NARROWING_CONVERSION`).

The script graph (§4) and this native table merge into **one** `Ty` lattice with a single `is_assignable`
(§3.3). Version the JSON per Godot release; it's the source of truth so the analyzer tracks engine versions
exactly. (Ingestion mechanics are the sibling note's job.)

---

## 7. Phased feasibility plan (Tier 0 → 3)

Difficulty/risk are relative (●=low … ●●●●●=high). "Value" = user-visible payoff per unit effort.

### Tier 0 — "Smart but shallow" (MVP). Difficulty ●● · Risk ● · Value ★★★★
Parse + per-file symbol table + syntactic completion. **No real type inference.** Completion is keyword +
by-name member completion sourced from `extension_api.json` (offer *all* members regardless of receiver type),
plus locals/params/script members in scope. Document outline, basic syntax diagnostics, signature help by name.
- *Unlocks:* the 80% of "it autocompletes and doesn't lie about syntax" value, immediately, with no type
  system. This is roughly parity with a decent existing LSP and is the right first ship.
- *Depends on:* parser (other note) + `extension_api.json` ingestion (sibling note).

### Tier 1 — Single-file type inference. Difficulty ●●● · Risk ●● · Value ★★★★★
The forward expression evaluator (§3.2): locals, `:=`, annotated vars/params/returns, literal/operator types,
member resolution against the **native** API and **same-file** classes/inner-classes/enums, `as` casts, basic
`is`/`!= null` narrowing. Hover shows inferred types; **type-aware completion** (only real members of the
receiver); diagnostics = the `UNSAFE_*` / `NARROWING_CONVERSION` / `INFERENCE_ON_VARIANT` family with verbatim
messages (§1.7).
- *Unlocks:* the leap from "shallow" to "actually understands your code" — the single biggest perceived-quality
  jump. Self-contained (no cross-file graph yet).
- *Risk:* getting the `Variant`-absorption and assignability rules right; matching Godot's warning semantics.

### Tier 2 — Project-wide resolution. Difficulty ●●●● · Risk ●●● · Value ★★★★
The cross-file graph (§4): scan `class_name`, resolve `extends`/`preload`/`load`, parse `[autoload]`. Enables
cross-file member resolution, **go-to-definition, find-references, rename, workspace symbols**, and
project-level diagnostics. Requires the incremental/salsa caching (§3.4, §4.5) to stay responsive.
- *Unlocks:* "IDE-grade navigation across the project." High value, but **most of the engineering complexity
  lives here** (caching correctness, invalidation, file-watching, `res://` mapping, cycle detection).
- *Risk:* incrementality bugs (stale results) are the classic failure mode; budget for it.

### Tier 3 — Scene-aware typing + full warnings + flow narrowing. Difficulty ●●●● · Risk ●●● · Value ★★★★★ (differentiator)
`.tscn` parsing → node-tree model → **typed `$`/`%`/`get_node()`** (§5); recursive instanced-scene typing;
`@onready` correctness; the *complete* warning set; and a real TypeScript-style CFG narrowing pass that
**beats** Godot's own `is`/`as` narrowing ([#93510](https://github.com/godotengine/godot/issues/93510)).
- *Unlocks:* the **killer feature** — node-path intelligence the built-in editor lacks. Highest differentiation.
- *Note:* scene typing (§5) can be **pulled forward** partially even at Tier 1.5 for the common single-scene
  case, because it doesn't strictly need the full project graph — only the owning scene + native API. Consider
  shipping a "direct `type=` node path" version early for outsized perceived value.

### Recommended highest-value-earliest path

**Tier 0 → Tier 1 → (early slice of Tier 3 scene typing) → Tier 2 → rest of Tier 3.**
Tier 0 ships fast and useful. Tier 1 delivers the biggest quality jump and is self-contained. A *thin* scene-
typing slice (direct `type=` nodes, single owning scene) can land right after Tier 1 for a wow-factor demo
before tackling the heavy project-graph incrementality of Tier 2. The genuinely hard, multi-year-flavored work
is **Tier 2's incremental correctness** and **Tier 3's full scene recursion + flow narrowing** — but even
GDScript's *hardest* tier is far below rust-analyzer's trait-solver/generics tier of difficulty
([DeepWiki: Type System and Inference](https://deepwiki.com/rust-lang/rust-analyzer/5.3-type-system-and-inference)).

---

## Sources

- Static typing in GDScript (stable): https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html
- Static typing in GDScript (4.4, typed dicts): https://docs.godotengine.org/en/4.4/tutorials/scripting/gdscript/static_typing.html
- GDScript reference / basics (scope, self, inner classes, enums, class_name, $/%, @onready): https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_basics.html
- GDScript warning system: https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/warning_system.html
- gdscript_warning.h (full Code enum): https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_warning.h
- gdscript_warning.cpp (exact messages): https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_warning.cpp
- TSCN file format (stable): https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html
- Singletons / Autoload (project.godot [autoload], `*` prefix): https://docs.godotengine.org/en/stable/tutorials/scripting/singletons_autoload.html
- rust-analyzer — Type System and Inference (DeepWiki): https://deepwiki.com/rust-lang/rust-analyzer/5.3-type-system-and-inference
- rust-analyzer — Semantic Analysis (DeepWiki): https://deepwiki.com/rust-lang/rust-analyzer/5-semantic-analysis
- rust-analyzer architecture.md (ItemTree/DefMap/Body): https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/architecture.md
- rust-analyzer — Durable Incrementality: https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html
- salsa (incremental query framework): https://github.com/salsa-rs/salsa
- TypeScript compiler architecture (binder/checker): https://zenn.dev/togami2864/articles/5b6c80cf913b7a?locale=en
- TypeScript binder (symbols/scopes/flow): https://basarat.gitbook.io/typescript/overview/binder
- GDExtension API / extension_api.json (DeepWiki): https://deepwiki.com/godotengine/godot/15.1-gdextension-api
- GDQuest — Type Inference (get_node/round Variant limits): https://www.gdquest.com/library/glossary/type_inference/
- Godot proposal #11056 — refactor GDScript Language Server: https://github.com/godotengine/godot-proposals/issues/11056
- Godot proposal #8530 / issue #93510 — is/as narrowing not suppressing unsafe warnings: https://github.com/godotengine/godot-proposals/issues/8530 , https://github.com/godotengine/godot/issues/93510
- global_script_class_cache.cfg bugs/proposals (#77478, #75388, #5631, PR #102636): https://github.com/godotengine/godot/issues/77478 , https://github.com/godotengine/godot/issues/75388 , https://github.com/godotengine/godot-proposals/issues/5631 , https://github.com/godotengine/godot/pull/102636
- Allen Pestaluky — enforcing static typing (warnings as errors): https://allenwp.com/blog/2023/10/03/how-to-enforce-static-typing-in-gdscript/
