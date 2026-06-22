# 04 — GDScript Semantics & Features: The Language Surface for a "Roslyn for Godot"

**Goal:** Define the *complete* language + semantic + IDE surface a Rust `gdscript-analyzer`
must parse and model to be a credible standalone "Roslyn for Godot."

**Target:** GDScript 2.0 / Godot 4.x. The `/en/stable/` docs currently render as **Godot 4.7**
in their page headers; the safe shipped reference is **4.3-stable / 4.4**, with **4.4** adding
typed dictionaries and mandatory class-cache keys, and **4.5** adding `@abstract` and varargs.
Version divergences are flagged inline.

**Research method:** Primary sources only — `docs.godotengine.org` (stable), the
`godotengine/godot` engine C++ source (`master` and `4.3-stable` tags), `godotengine/godot-docs`,
`godotengine/godot-proposals`, the `godot-vscode-plugin` repo, and real `project.godot` fixtures.
All URLs cited inline and in the index at the bottom.

---

## 1. GDScript Language Constructs — Complete Grammar-Surface Checklist

Source: [GDScript reference (stable/4.7)](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_basics.html),
[static typing](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html),
[@GDScript class ref (4.7)](https://docs.godotengine.org/en/stable/classes/class_@gdscript.html).

### 1.1 Keywords (full set)
- Control flow: `if`, `elif`, `else`, `for`, `while`, `match`, `when` (match guard), `break`, `continue`, `pass`, `return`
- Declarations: `var`, `const`, `enum`, `func`, `static`, `signal`, `class`, `class_name`, `extends`
- Type/scope ops: `is`, `is not`, `in`, `not in`, `as`, `self`, `super`, `void`
- Async/special: `await`, `preload`, `assert`, `breakpoint`, `yield` (deprecated — emits `DEPRECATED_KEYWORD`)
- Built-in constants: `true`, `false`, `null`, `PI`, `TAU`, `INF`, `NAN`

### 1.2 Variables & constants
- [ ] `var a = 5` — untyped (emits `UNTYPED_DECLARATION`, default IGNORE)
- [ ] `var x: int` — explicitly typed
- [ ] `var x := 1` — inferred type (`INFERRED_DECLARATION`, default IGNORE; `INFERENCE_ON_VARIANT` = ERROR if RHS is Variant)
- [ ] `const ANSWER = 42`, `const X: int = 42`, `const X := 42`
- [ ] `static var a` (Godot 4.x) — class-level, shared across instances; pairs with `@static_unload` / `_static_init()`
- [ ] `var x: Array[int]`, `var x: Array[Node]` — typed arrays
- [ ] `var x: Dictionary[String, int]` — **typed dictionaries (4.4+)**. Nested generics (`Array[Array[int]]`) are **not** allowed.
- [ ] Property accessors (4.x setget replacement) — inline or named:
  ```gdscript
  var health: int = 100:
      get:
          return health
      set(value):
          health = clamp(value, 0, 100)
  # or reference named functions:
  var health: int = 100: get = get_health, set = set_health
  ```

### 1.3 Functions
- [ ] `func f(a, b, c = 123):` — params with default args
- [ ] `func f(a: int, b: String) -> int:` — typed params + return; `-> void` for no return
- [ ] `func f(int_arg := 42, s_arg := "x"):` — inferred param types
- [ ] `static func f(a, b):` — static method
- [ ] `func square(a): return a * a` — single-line body
- [ ] Lambdas: `var l = func(x): print(x)`; typed: `var l := func(x: int) -> void: print(x)`. Lambdas capture by value; reassigning a capture emits `CONFUSABLE_CAPTURE_REASSIGNMENT`.
- [ ] **Varargs (4.5+):** `func f(a, b = 0, ...args):` — rest parameter collects extras into an Array
- [ ] `_init()` constructor; `static func _static_init():` static constructor
- [ ] `super(args)` (parent constructor) and `super.method()` (parent method)
- [ ] Lifecycle virtuals to model as overridable: `_init`, `_ready`, `_process`, `_physics_process`, `_enter_tree`, `_exit_tree`, `_input`, `_unhandled_input`, `_draw`, `_notification`, `_get`, `_set`, `_get_property_list`, etc. Overriding a *native* method incorrectly emits `NATIVE_METHOD_OVERRIDE` (ERROR).

### 1.4 Types (built-ins — full set the analyzer must know natively)
- Primitives: `null`, `bool`, `int`, `float`, `String`, `StringName`, `NodePath`
- Vector/math: `Vector2`, `Vector2i`, `Vector3`, `Vector3i`, `Vector4`, `Vector4i`, `Rect2`, `Rect2i`, `Transform2D`, `Plane`, `Quaternion`, `AABB`, `Basis`, `Transform3D`, `Projection`, `Color`
- Engine/handle: `RID`, `Object`, `Callable`, `Signal`
- Collections: `Array`, typed `Array[T]`, `Dictionary`, typed `Dictionary[K,V]` (4.4+)
- Packed arrays: `PackedByteArray`, `PackedInt32Array`, `PackedInt64Array`, `PackedFloat32Array`, `PackedFloat64Array`, `PackedStringArray`, `PackedVector2Array`, `PackedVector3Array`, `PackedVector4Array`, `PackedColorArray`
- `Variant` — the any-type (source of all `UNSAFE_*` warnings)
- **Every engine class** (`Node`, `Node2D`, `Resource`, `Control`, …) — the analyzer must ingest the engine API (from `extension_api.json` / the docs XML) as built-in types.

### 1.5 Classes & inheritance
- [ ] `class_name MyClass` (optionally `class_name MyClass extends Base` one-liner; optionally preceded by `@icon("res://...")`) — registers a **global** type (see §3.1)
- [ ] `extends Base` (global class or engine type), `extends "res://x.gd"` (path), `extends "x.gd".InnerClass`
- [ ] Default base is `RefCounted` if `extends` omitted
- [ ] `class Inner:` — inner classes (NOT globally registered; reached as `Outer.Inner`); `Outer.Inner.new()`
- [ ] `@abstract class Shape:` / `@abstract func draw()` (**4.5+**) — abstract classes/methods; instantiating an abstract class is a hard **error** (not a warning)

### 1.6 Enums, signals
- [ ] Unnamed enum: `enum {UNIT_NEUTRAL, UNIT_ENEMY, UNIT_ALLY}` (injects constants into the class)
- [ ] Named enum: `enum Named {THING_1, THING_2, ANOTHER = -1}` (a named type; usable as `var x: Named`)
- [ ] `signal my_signal` and typed `signal my_signal(arg1: int, arg2: String)`; unused → `UNUSED_SIGNAL`
- [ ] Signals are awaitable: `await obj.my_signal`; emit via `my_signal.emit(...)`; `Signal`/`Callable` are first-class types

### 1.7 Control flow
- [ ] `if / elif / else`
- [ ] `for x in <iterable>:`, typed `for name: String in names:`, `for i in range(a, b, step):`
- [ ] `while cond:`
- [ ] `match` with pattern types: literals, expressions, wildcard `_`, **binding** `var name`, **array** `[1, 3, "x"]`, **dictionary** `{"name": "Dennis"}`, **open-ended** `[1, ..]` / `{"k": v, ..}`, multiple comma-separated patterns, and **guards** `[var x, var y] when y == x:`. Match is stricter than `==` (String/StringName are equivalent). `UNREACHABLE_PATTERN` for patterns after a wildcard/bind.
- [ ] `break`, `continue`, `pass`, `return`

### 1.8 Expressions & operators
- Arithmetic `+ - * / % **`; integer `/` of two ints → `INTEGER_DIVISION` warning; float→int → `NARROWING_CONVERSION`
- Comparison `== != < > <= >=`; logical `and/&&`, `or/||`, `not/!`; bitwise `~ & | ^ << >>`; all compound-assign forms (`+= -= *= /= **= %= &= |= ^= <<= >>=`)
- Type ops: `is`, `is not`, `as` (cast — object cast yields `null` on mismatch silently; Variant cast → `UNSAFE_CAST`), `in`, `not in`
- Ternary: `true_expr if cond else false_expr` (right-associative); standalone → `STANDALONE_TERNARY`; mismatched arms → `INCOMPATIBLE_TERNARY`
- `await expr` (coroutine or signal); redundant → `REDUNDANT_AWAIT`; missing on a coroutine → `MISSING_AWAIT` (master only)
- Subscription `x[i]`, attribute `x.y`, call `f()`
- Literals: int (`45`, `0x8f51`, `0b101010`, `12_345`), float (`3.14`, `58.1e-10`), strings (`"..."`, `'...'`, `"""..."""`, raw `r"..."`), bool, null, array `[...]`, dict `{k: v}` and Lua-style `{key = v}`

### 1.9 Special syntax & node access
- [ ] `&"StringName"` literal; `^"Node/Path"` NodePath literal
- [ ] `$NodePath` ≡ `get_node("NodePath")`; `$"Path/With Spaces"`; `%UniqueName` ≡ `get_node("%UniqueName")` (see §4)
- [ ] `get_node(path)`, `get_node_or_null`
- [ ] `preload("res://x.gd")` (parse-time keyword, const path only) vs `load("res://x.gd")` (runtime call)
- [ ] `const X = preload("res://x.gd")` — the key static hook for cross-file type resolution
- [ ] `assert(cond, msg)` (stripped in release; `ASSERT_ALWAYS_TRUE/FALSE` warnings)
- [ ] `self`, `super`
- [ ] String formatting via `%` operator and `String.format`

### 1.10 Annotations — COMPLETE list (36, from the [@GDScript 4.7 reference](https://docs.godotengine.org/en/stable/classes/class_@gdscript.html))
Each must be recognized with its arity so the analyzer can validate placement, suggest in completion, and apply semantic effects (export → inspector property; onready → deferred init; rpc → networked).

| # | Annotation | Signature |
|---|---|---|
| 1 | `@abstract` | `@abstract()` — class/method abstract (4.5+) |
| 2 | `@export` | `@export()` |
| 3 | `@export_category` | `@export_category(name: String)` |
| 4 | `@export_color_no_alpha` | `@export_color_no_alpha()` |
| 5 | `@export_custom` | `@export_custom(hint: PropertyHint, hint_string: String, usage: BitField[PropertyUsageFlags] = 6)` |
| 6 | `@export_dir` | `@export_dir()` — project-relative dir |
| 7 | `@export_enum` | `@export_enum(names: String, ...) vararg` |
| 8 | `@export_exp_easing` | `@export_exp_easing(hints: String = "", ...) vararg` |
| 9 | `@export_file` | `@export_file(filter: String = "", ...) vararg` |
| 10 | `@export_file_path` | `@export_file_path(filter: String = "", ...) vararg` — raw file path |
| 11 | `@export_flags` | `@export_flags(names: String, ...) vararg` |
| 12 | `@export_flags_2d_navigation` | `@export_flags_2d_navigation()` |
| 13 | `@export_flags_2d_physics` | `@export_flags_2d_physics()` |
| 14 | `@export_flags_2d_render` | `@export_flags_2d_render()` |
| 15 | `@export_flags_3d_navigation` | `@export_flags_3d_navigation()` |
| 16 | `@export_flags_3d_physics` | `@export_flags_3d_physics()` |
| 17 | `@export_flags_3d_render` | `@export_flags_3d_render()` |
| 18 | `@export_flags_avoidance` | `@export_flags_avoidance()` |
| 19 | `@export_global_dir` | `@export_global_dir()` — absolute dir |
| 20 | `@export_global_file` | `@export_global_file(filter: String = "", ...) vararg` — absolute file |
| 21 | `@export_group` | `@export_group(name: String, prefix: String = "")` |
| 22 | `@export_multiline` | `@export_multiline(hint: String = "", ...) vararg` |
| 23 | `@export_node_path` | `@export_node_path(type: String = "", ...) vararg` |
| 24 | `@export_placeholder` | `@export_placeholder(placeholder: String)` |
| 25 | `@export_range` | `@export_range(min: float, max: float, step: float = 1.0, extra_hints: String = "", ...) vararg` |
| 26 | `@export_storage` | `@export_storage()` — serialized but hidden from inspector |
| 27 | `@export_subgroup` | `@export_subgroup(name: String, prefix: String = "")` |
| 28 | `@export_tool_button` | `@export_tool_button(text: String, icon: String = "")` — exports a `Callable` as inspector button |
| 29 | `@icon` | `@icon(icon_path: String)` |
| 30 | `@onready` | `@onready()` — defers assignment to just before `_ready()` (combining with `@export` → `ONREADY_WITH_EXPORT` ERROR; `get_node` default without `@onready` → `GET_NODE_DEFAULT_WITHOUT_ONREADY` ERROR) |
| 31 | `@rpc` | `@rpc(mode = "authority", sync = "call_remote", transfer_mode = "reliable", transfer_channel: int = 0)` |
| 32 | `@static_unload` | `@static_unload()` — redundant if no static vars → `REDUNDANT_STATIC_UNLOAD` |
| 33 | `@tool` | `@tool()` — runs script in editor; base `@tool` w/o local `@tool` → `MISSING_TOOL` |
| 34 | `@warning_ignore` | `@warning_ignore(warning: String, ...) vararg` |
| 35 | `@warning_ignore_restore` | `@warning_ignore_restore(warning: String, ...) vararg` |
| 36 | `@warning_ignore_start` | `@warning_ignore_start(warning: String, ...) vararg` |

> Note: docs prose sometimes lists `@export_node_path("Button","TouchScreenButton")` etc.; the class-ref signatures above are authoritative for arity. `@export_file_path` is the raw-path variant of `@export_file`. `@export_placeholder` IS present (despite the older exports tutorial page omitting it).

### 1.11 Comments / docs / regions (lexer-level, analyzer should surface)
- `#` comment; `##` documentation comment (feeds hover/inspector docs); `#region NAME … #endregion` foldable; special highlighted keywords (`TODO`, `FIXME`, `HACK`, `DEPRECATED`, `BUG`, `NOTE`, …). Line continuation with trailing `\`.

---

## 2. Type System & Static Analysis (the hard part)

Sources: [static typing (4.7)](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html),
[warning system (4.7)](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/warning_system.html),
[`gdscript_warning.h` (master)](https://raw.githubusercontent.com/godotengine/godot/master/modules/gdscript/gdscript_warning.h),
[`gdscript_warning.cpp` (master)](https://raw.githubusercontent.com/godotengine/godot/master/modules/gdscript/gdscript_warning.cpp),
[`gdscript.cpp` (master)](https://raw.githubusercontent.com/godotengine/godot/master/modules/gdscript/gdscript.cpp).

### 2.1 Gradual/optional typing model
- Typing is **opt-in**; typed and dynamic code coexist. Untyped behaves dynamically.
- `: Type` annotations on var/const/param; `-> Type` return; `void` is return-position only.
- **Inference `:=`** infers from the initializer's *static* type. Inferring from a `Variant`
  → `INFERENCE_ON_VARIANT` (default **ERROR**).
- **Variant** is the any-type. Any property/method access *through* Variant is "unsafe."
- **Safe lines:** the editor marks line numbers green when the type checker can statically
  prove the operation (no implicit Variant round-trip). `as` makes a line "safe" while silently
  yielding `null` on object-type mismatch — a documented footgun; the recommended safe pattern is
  an `is` guard that narrows to a typed local.
- **Typed arrays / dictionaries** enforce element/key/value types at runtime and propagate to
  `for` loop variables, `[]`, indexed assignment, and `+`. Nested generics unsupported.
- **Duck typing** still works in dynamic code; restore safety with `if node is T: var t: T = node`.

### 2.2 COMPLETE warning list (authoritative — from `gdscript_warning.h` `master`)
48 codes on master (47 on 4.3-stable). Default level from `default_warning_levels[]`: `WARN` | `IGNORE` | `ERROR`.

**WARN by default**
- [ ] `UNASSIGNED_VARIABLE`
- [ ] `UNASSIGNED_VARIABLE_OP_ASSIGN`
- [ ] `UNUSED_VARIABLE`
- [ ] `UNUSED_LOCAL_CONSTANT`
- [ ] `UNUSED_PRIVATE_CLASS_VARIABLE`
- [ ] `UNUSED_PARAMETER`
- [ ] `UNUSED_SIGNAL`
- [ ] `SHADOWED_VARIABLE`
- [ ] `SHADOWED_VARIABLE_BASE_CLASS`
- [ ] `SHADOWED_GLOBAL_IDENTIFIER`
- [ ] `UNREACHABLE_CODE`
- [ ] `UNREACHABLE_PATTERN`
- [ ] `STANDALONE_EXPRESSION`
- [ ] `STANDALONE_TERNARY`
- [ ] `INCOMPATIBLE_TERNARY`
- [ ] `UNSAFE_VOID_RETURN`
- [ ] `STATIC_CALLED_ON_INSTANCE`
- [ ] `MISSING_TOOL`
- [ ] `REDUNDANT_STATIC_UNLOAD`
- [ ] `REDUNDANT_AWAIT`
- [ ] `ASSERT_ALWAYS_TRUE`
- [ ] `ASSERT_ALWAYS_FALSE`
- [ ] `INTEGER_DIVISION`
- [ ] `NARROWING_CONVERSION`
- [ ] `INT_AS_ENUM_WITHOUT_CAST`
- [ ] `INT_AS_ENUM_WITHOUT_MATCH`
- [ ] `ENUM_VARIABLE_WITHOUT_DEFAULT`
- [ ] `EMPTY_FILE`
- [ ] `DEPRECATED_KEYWORD`
- [ ] `CONFUSABLE_IDENTIFIER`
- [ ] `CONFUSABLE_LOCAL_DECLARATION`
- [ ] `CONFUSABLE_LOCAL_USAGE`
- [ ] `CONFUSABLE_CAPTURE_REASSIGNMENT`
- [ ] `CONFUSABLE_TEMPORARY_MODIFICATION` *(master only; not in 4.3)*
- [ ] `PROPERTY_USED_AS_FUNCTION` *(deprecated; compiled out under `DISABLE_DEPRECATED`)*
- [ ] `CONSTANT_USED_AS_FUNCTION` *(deprecated)*
- [ ] `FUNCTION_USED_AS_PROPERTY` *(deprecated)*

**IGNORE by default (the type-strictness opt-in group)**
- [ ] `UNTYPED_DECLARATION`
- [ ] `INFERRED_DECLARATION`
- [ ] `UNSAFE_PROPERTY_ACCESS`
- [ ] `UNSAFE_METHOD_ACCESS`
- [ ] `UNSAFE_CAST`
- [ ] `UNSAFE_CALL_ARGUMENT`
- [ ] `RETURN_VALUE_DISCARDED`
- [ ] `MISSING_AWAIT` *(master only; not in 4.3)*

**ERROR by default (hard-fail out of the box)**
- [ ] `INFERENCE_ON_VARIANT`
- [ ] `NATIVE_METHOD_OVERRIDE`
- [ ] `GET_NODE_DEFAULT_WITHOUT_ONREADY`
- [ ] `ONREADY_WITH_EXPORT`

**Not warnings (do not model as warnings):**
- `ABSTRACT_CLASS_INSTANTIATED` — a hard **error**, not a configurable warning.
- `RENAMED_IN_GODOT_4_HINT` — exists only ≤ 4.3 (registered as a BOOL setting); removed on master.

> Severity ordering and integer enum values shifted between 4.3 and master — key on the
> **symbolic name / setting path**, never the integer.

### 2.3 Warning configuration (project settings the analyzer must read)
Setting path: `debug/gdscript/warnings/<lowercase_name>`, a 3-way enum `Ignore(0),Warn(1),Error(2)`.
Global toggles:
- [ ] `debug/gdscript/warnings/enable` (bool, default `true`) — master switch
- [ ] `debug/gdscript/warnings/treat_warnings_as_errors` (bool) — escalate all WARN→ERROR
- [ ] `debug/gdscript/warnings/exclude_addons` (bool, default `true`) — *4.3/4.4*; suppress `res://addons/...`
- [ ] `debug/gdscript/warnings/directory_rules` (dictionary) — *master only*; per-dir Include/Exclude, `res://addons` excluded by default (replaces `exclude_addons`)
- [ ] per-warning keys `debug/gdscript/warnings/<name>` (one per code above)

Inline suppression: `@warning_ignore("name")` (one line/decl); `@warning_ignore_start("name")` …
`@warning_ignore_restore("name")` (region). Names are the lowercased setting names.

---

## 3. Project / Script Resolution Model

This is the cross-file graph the analyzer must reconstruct. Five linking mechanisms:

| Mechanism | Binds | Resolved | Source of truth |
|---|---|---|---|
| `class_name` global registry | bare type name → script file | parse-time, project-wide | `*.gd` (cached `.godot/global_script_class_cache.cfg`) |
| `preload`/`load`/`extends "path"` | const/base → script Resource | preload/extends/const = parse-time; load = runtime | the `res://` path string |
| `[autoload]` singletons | bare identifier → global node | parse-time (read project.godot) | `project.godot` `[autoload]` |
| `.tscn` tree + `ext_resource` script | `$Node`/`%Unique` → node class/script | parse-time (read scene) | `*.tscn` / `*.tres` |
| project settings | warnings, main scene, input, groups | config | `project.godot` |

### 3.1 `class_name` global registry
- `class_name MyClass` registers the file's **main** class globally (no preload needed). Only the
  top-level class — inner `class` are NOT global (reached as `Outer.Inner`).
- On-disk cache: `res://.godot/global_script_class_cache.cfg` — a Godot `ConfigFile` with one key
  `list` = Array of Dictionaries. **Seven keys** per entry (4.4+): `class`, `language`, `path`,
  `base`, `icon`, `is_abstract`, `is_tool`. Path constant: `ProjectSettings::get_global_class_list_path()`
  in [`core/config/project_settings.cpp`](https://github.com/godotengine/godot/blob/master/core/config/project_settings.cpp);
  registry is `ScriptServer` in [`core/object/script_language.cpp`](https://github.com/godotengine/godot/blob/master/core/object/script_language.cpp).
- **4.4 caveat:** `is_abstract`/`is_tool` became mandatory; pre-4.4 caches are silently skipped on
  upgrade (issue [#102568](https://github.com/godotengine/godot/issues/102568), fix PR
  [#102636](https://github.com/godotengine/godot/pull/102636)). Globals were moved from `project.godot`
  into `.godot/` per PR #70557 / proposal #5631.
- **Analyzer implication:** do NOT trust the cache; parse `class_name` directly from `*.gd` (or
  regenerate), especially across engine versions.

### 3.2 preload / load / extends, and `res://` resolution
- `preload(const String)` — parse-time keyword, **constant** path only; acts as a reference to the
  resource. `load(String)` — runtime, dynamic path (≡ `ResourceLoader.load`). Both are `@GDScript`
  members in 4.x. ([@GDScript ref](https://docs.godotengine.org/en/stable/classes/class_@gdscript.html))
- `const X = preload("res://x.gd")` is the analyzer's key static hook — `var v: X`, `v is X`, and
  `X.Inner` all resolve.
- `res://` = the project root (dir containing `project.godot`). `user://` = writable per-user dir.
- `extends` forms: `extends GlobalClass`, `extends "res://x.gd"`, `extends "res://x.gd".Inner`.
  Default base is `RefCounted`.

### 3.3 Autoloads / singletons
- `project.godot` `[autoload]` section: `Name="*res://path.gd_or_tscn"`. The leading `*` →
  `is_singleton = true` (parsed in `project_settings.cpp`), meaning the name is a **global GDScript
  identifier** (`PlayerVariables.health -= 10`). No `*` → autoloaded node only, reachable via
  `get_node("/root/Name")`, NOT a global identifier.
- The GDScript front-end gates the global on `is_singleton`
  ([`gdscript_analyzer.cpp`](https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_analyzer.cpp));
  a `class_name` colliding with an autoload → error "hides an autoload singleton."
- Autoloads ≠ engine singletons (`OS`, `Input`, `RenderingServer`, tracked separately via `Engine.get_singleton`).
- **Analyzer:** parse `[autoload]`, strip `*`, seed the global identifier table with each singleton
  name → type of the script/scene-root at that path.

### 3.4 Scenes (`.tscn`) and node-path typing
- TSCN text format; header `[gd_scene load_steps=N format=3 uid="uid://..."]`. **`format=3` = 4.x**
  (the reliable version discriminator). `load_steps` advisory (deprecated on 4.6+).
  ([TSCN format docs](https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html))
- A node's attached **script** is an `ext_resource type="Script"` referenced by the node's `script`
  property: `script = ExtResource("id")` → upgrades the node's static type from its engine class to
  its **script class**. This is the crux of typing `$Node` correctly.
- Node tree encoding: `[node name="..." type="..." parent="..." index="..." groups=[...] instance=ExtResource("...")]`.
  Exactly one root (no `parent=`); `parent="."` = child of root; nested via relative path (root name excluded).
- Instanced sub-scenes: `[node name="X" parent="." instance=ExtResource("id")]` — **no `type=`**;
  class/script/children come from the referenced `PackedScene` (follow it). `instance_placeholder=...` for lazy load.
- `$Path` ≡ `get_node("Path")`; `..` = parent, leading `/` = absolute (`/root/...`); quoted `$"With Spaces"`.
- `@onready var x = $Path` defers *assignment* but the **type is the resolved node's** — resolve at decl site.
- Unique names: `%Name` ≡ `get_node("%Name")`, resolves to the node with `unique_name_in_owner = true`
  within the current scene's owner (inside an instanced sub-scene, within that sub-scene's owner).
- **What to extract per `.tscn`:** (1) format/version; (2) node table `name`/`type`/`parent`;
  (3) parent relationships → tree; (4) attached scripts (`script = ExtResource`); (5)
  `unique_name_in_owner = true` map; (6) instanced sub-scenes (follow referenced `.tscn`).
  (2)+(3)+(4) types `$Node`; (2)+(3)+(6) validates a path; (5)+(6) resolves `%Unique`.

### 3.5 Resources (`.tres`/`.res`) and `@tool`
- `.tres` (text) / `.res` (binary) share the scene grammar: `[gd_resource ...]`, `[ext_resource ...]`,
  `[sub_resource ...]`. Scripts referenced the same way.
- `@tool` runs a script in-editor; `Engine.is_editor_hint()` branches editor-only code; tool-script
  deps must also be tool. For the analyzer, mostly a flag (`is_tool` cache key).

### 3.6 `project.godot` format
Plain-text **ConfigFile / INI**; `[section]` headers; `key=value` where **values are typed Variants**
(`VariantWriter` encoding: ints, quoted strings, `Vector3(...)`, `Object(InputEventKey,...)`); `;` comments;
slash keys split into section+key (`application/config/name` → `[application] config/name`).
**`config_version=5` = 4.x.** Sections to parse: `[application]` (`run/main_scene`), `[autoload]`,
`[debug]` (`gdscript/warnings/*`), `[input]` (action → `{deadzone, events:[Object(InputEvent...)]}`),
`[global_group]` (optional; only present when global groups exist).

### 3.7 Authoritative vs derived files (symbol-graph inputs)
**Authoritative (parse these):** `project.godot`, `*.gd`, `*.tscn`, `*.tres`, `*.import` (per-asset).
**Derived / regenerable (do NOT trust as truth):** `.godot/global_script_class_cache.cfg`
(class_name index — can be stale), `.godot/uid_cache.bin` (UID↔path; truth is `uid="uid://..."`
strings in the files), `.godot/imported/`, `.godot/editor/`. Robust design rebuilds `class_name`
globals + `uid://` maps from the authoritative files.

---

## 4. IDE Feature Set → LSP Mapping (with difficulty & required semantic data)

Difficulty is for a *correct* implementation over gradually-typed code. Semantic data column =
what must already exist in the analyzer's model.

| LSP feature | Method | Difficulty | Requires | MVP? |
|---|---|---|---|---|
| Diagnostics: parse errors | `publishDiagnostics` | Low | Lexer + parser w/ recovery | **MVP** |
| Diagnostics: type errors + lints | `publishDiagnostics` | **High** | Full type checker + the 48 warnings + project settings | **MVP (core subset)** |
| Document symbols | `documentSymbol` | Low | AST + scopes | **MVP** |
| Folding ranges | `foldingRange` | Low | AST + `#region` lexer tokens | **MVP** |
| Hover | `hover` | Medium | Symbol resolution + type of expr + `##` docs + engine docs | **MVP** |
| Completion: keywords/snippets | `completion` | Low | Keyword table | **MVP** |
| Completion: members (`.`) | `completion` | **High** | Type inference of receiver + class member tables (incl. engine API) | **MVP** |
| Completion: globals/types | `completion` | Medium | Global registry (`class_name`, autoloads, engine types) | **MVP** |
| Completion: annotations (`@`) | `completion` | Low | The 36-annotation table | **MVP** |
| Completion: paths/node-paths | `completion` | **High** | `res://` FS index + scene tree model for `$`/`%` | Later |
| Completion: signals / `@warning_ignore` names | `completion` | Medium | Signal tables / warning-name table | Later |
| Go-to-definition | `definition` | Medium-High | Cross-file symbol graph (preload/extends/class_name/autoload) | **MVP** |
| Go-to-declaration / type-def / impl | `declaration` / `typeDefinition` / `implementation` | Medium | Same graph + type-of-symbol | Later |
| Signature help | `signatureHelp` | Medium | Function signatures (incl. defaults/varargs) + active-arg tracking | **MVP** |
| Find references | `references` | **High** | Project-wide index + name resolution (the dynamic-typing hazard) | Phase 2 |
| Rename | `rename` / `prepareRename` | **Very High** | Accurate references incl. local scope, scenes, `class_name` usages | Phase 2 |
| Workspace symbols | `workspaceSymbol` | Medium | Project-wide symbol index | Phase 2 |
| Semantic tokens | `semanticTokens/full` | Medium-High | Resolved kind per identifier (var/member/type/param/enum/signal) | Phase 2 (differentiator) |
| Inlay hints (inferred types!) | `inlayHint` | **High** | Inferred static type at each `:=`/untyped decl + param-name hints | Phase 2 (differentiator) |
| Code actions / quick-fixes | `codeAction` | High | Diagnostics + edits (add type, prefix `_`, add `@onready`, cast) | Phase 2 |
| Formatting | `formatting` / `rangeFormatting` | Medium | Lossless CST / token stream + style rules | Phase 2 |
| Document highlight | `documentHighlight` | Low-Medium | Local name resolution | Later |
| Call hierarchy | `callHierarchy/*` | High | Project-wide call graph | Phase 3 |
| Document links | `documentLink` | Low | `res://` path literals → files | Later |

**The three hardest semantic problems:**
1. **Type inference over gradual typing** — propagate types through `:=`, member access, returns,
   typed/untyped boundaries, and Variant, well enough that member completion + diagnostics are useful
   on real (largely untyped) projects. This is the quality ceiling Godot's own LSP hits and the main bar to beat.
2. **Project-wide resolution** — building and incrementally maintaining the cross-file symbol graph
   (class_name globals, preload/extends chains, autoloads) that go-to-def / references / rename need.
3. **Scene / node-path typing** — parsing `.tscn` to type `$Node`/`%Unique` and validate paths;
   following instanced sub-scenes and `unique_name_in_owner`. Unique to Godot; nothing in Roslyn-land
   maps to it.

---

## 5. Godot's Built-in GDScript LSP — the Bar and the Gaps

Sources: [`godot_lsp.h` (master)](https://github.com/godotengine/godot/blob/master/modules/gdscript/language_server/godot_lsp.h),
[`gdscript_language_protocol.cpp`](https://github.com/godotengine/godot/blob/master/modules/gdscript/language_server/gdscript_language_protocol.cpp),
[`gdscript_text_document.cpp`](https://github.com/godotengine/godot/blob/master/modules/gdscript/language_server/gdscript_text_document.cpp),
[external editor docs](https://docs.godotengine.org/en/stable/tutorials/editor/external_editor.html),
[godot-vscode-plugin](https://github.com/godotengine/godot-vscode-plugin).

### 5.1 What the built-in LSP supports (verified from the capabilities struct)
| Capability | Status |
|---|---|
| `textDocumentSync`, `completion`(+resolve), `hover`, `signatureHelp` | Implemented |
| `definition`, `declaration`, `documentSymbol`, `documentHighlight`, `documentLink` | Implemented |
| `references`, `rename`(+`prepareRename`) | **Implemented since Godot 4.2** (PR [#80973](https://github.com/godotengine/godot/pull/80973)) |
| `codeLens`, `colorPresentation`, `foldingRange` | **Stubs** (return empty arrays) |
| `nativeSymbol` | Godot-specific (engine docs) |
| `typeDefinition`, `implementation`, `workspaceSymbol`, `codeAction`, `formatting` | **`false` (not provided)** |
| `semanticTokens`, `inlayHint` | **Absent from the struct entirely** |

### 5.2 Gaps / limitations (cited)
- **References/rename are recent and buggy.** No `references` before 4.2 (proposal
  [#3687](https://github.com/godotengine/godot-proposals/issues/3687)). Rename breaks for
  method-local vars: [#76094](https://github.com/godotengine/godot/issues/76094). File rename
  doesn't update `.uid`: [#105515](https://github.com/godotengine/godot/issues/105515). Standing
  demand for real refactor tooling: proposal [#899](https://github.com/godotengine/godot-proposals/issues/899),
  discussions [#7952](https://github.com/godotengine/godot-proposals/discussions/7952) / [#8463](https://github.com/godotengine/godot-proposals/discussions/8463).
- **No semantic tokens** — absent from struct; highlighting relies on TextMate grammars
  (member-var coloring is open proposal [#6428](https://github.com/godotengine/godot-proposals/issues/6428)).
- **No inlay hints** — absent; requested in discussion [#7683](https://github.com/godotengine/godot-proposals/discussions/7683), deflected, unimplemented.
- **Completion gaps tied to dynamic typing** — plugin FAQ: *"The language server can't infer all
  variable types… use static typing"* to get more results. Doesn't insert `()`
  ([#84549](https://github.com/godotengine/godot/issues/84549)); inconsistent across clients
  ([#93224](https://github.com/godotengine/godot/issues/93224)).
- **Stale results / restart-the-LSP** workarounds common; "Use Thread" editor setting recommended.
- **Go-to-definition limits** — 4.5 regression [#111400](https://github.com/godotengine/godot/issues/111400)
  (fixed 4.6); fails for autoloads-as-type-annotations [#109706](https://github.com/godotengine/godot/issues/109706);
  VSCode jump-to-def issues [#380](https://github.com/godotengine/godot-vscode-plugin/issues/380).
- **Not LSP-spec compliant** — proposal [#11056](https://github.com/godotengine/godot-proposals/issues/11056):
  the server *"allows multiple connections… which the specification forbids,"* blocking client-capability
  tracking and modern features; describes the code as fragile hand-rolled serialization; proposes a
  separate headless `--gdscript-lsp` process (unimplemented).

### 5.3 The architectural constraint (the central differentiator)
- **The LSP is hosted inside the running Godot editor**, TCP **port 6005** (localhost). Docs:
  *"a Godot instance must be running on your current project."* Plugin FAQ: *"open the project in the
  Godot editor first."*
- **No standalone language-server binary.** "Headless" still spawns a full editor process with
  `--headless` (plugin PR [#488](https://github.com/godotengine/godot-vscode-plugin/pull/488)); it
  added `--lsp-port`. There is no engine-independent analyzer.
- **Consequence:** analysis quality is capped by the engine's gradually-typed inference; no
  independent project-wide static index; references/rename are best-effort.

**The bar to beat / the gap to fill:** a Rust `gdscript-analyzer` that (1) runs **without the engine**
(standalone, CI-friendly, no port 6005); (2) is **spec-compliant** (single connection → client
capabilities, snippets); (3) ships **semantic tokens** and **inlay hints** (absent today);
(4) provides **reliable project-wide references / rename / workspace symbols / code actions**;
(5) has **stronger inference over untyped code** than the engine; (6) stays fresh **without
save/restart**. Accurate framing: references/rename DO exist (since 4.2) but are recent, incomplete,
dynamic-typing-limited, locally buggy, and structurally constrained by the editor-hosted,
non-spec-compliant architecture.

---

## Citation Index

**Docs (docs.godotengine.org/en/stable):** `tutorials/scripting/gdscript/gdscript_basics.html` ·
`tutorials/scripting/gdscript/static_typing.html` · `tutorials/scripting/gdscript/warning_system.html` ·
`tutorials/scripting/gdscript/gdscript_exports.html` · `classes/class_@gdscript.html` (annotations, 4.7) ·
`tutorials/io/data_paths.html` · `tutorials/scripting/singletons_autoload.html` · `classes/class_engine.html` ·
`engine_details/file_formats/tscn.html` · `tutorials/scripting/nodes_and_scene_instances.html` ·
`tutorials/scripting/scene_unique_nodes.html` · `tutorials/editor/project_settings.html` ·
`classes/class_configfile.html` · `tutorials/plugins/running_code_in_the_editor.html` ·
`tutorials/best_practices/version_control_systems.html` · `tutorials/assets_pipeline/import_process.html` ·
`tutorials/editor/external_editor.html`

**Engine source (github.com/godotengine/godot):** `modules/gdscript/gdscript_warning.h` · `gdscript_warning.cpp` ·
`gdscript.cpp` · `gdscript_analyzer.cpp` · `gdscript_compiler.cpp` · `language_server/godot_lsp.h` ·
`language_server/gdscript_language_protocol.cpp` · `language_server/gdscript_text_document.cpp` ·
`core/config/project_settings.cpp/.h` · `core/object/script_language.cpp/.h` · `core/io/config_file.cpp` ·
`editor/file_system/editor_file_system.cpp` · `scene/resources/resource_format_text.cpp` ·
`scene/main/node.h` · `modules/gdscript/doc_classes/@GDScript.xml` (compared at `master` and `4.3-stable` tags)

**Proposals / issues / plugin:** godot-proposals #3687, #899, #7952, #8463, #6428, #7683, #11056, #9146, #5631 ·
godot #76094, #105515, #84549, #93224, #111400, #109706, #102568, #58330 · godot PR #80973, #102636, #70557 ·
godot-vscode-plugin (repo + PR #488, #511, issues #380, #455, #473, #758)

**Real fixtures verified:** `godot-demo-projects` dodge_the_creeps `project.godot` (`config_version=5`,
`[input]` `Object(InputEventKey,...)`); `Maaack/Godot-Game-Template` `project.godot` (`[autoload]` `*res://...`).
