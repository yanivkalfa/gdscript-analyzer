# 03 — Godot Engine API Ingestion & Sync (`extension_api.json` + doc XML)

**Status:** research note (data-ingestion + sync-process design)
**Date:** 2026-06-22
**Owner constraint (load-bearing, #1 priority):** the analyzer must have **authoritative,
always-fresh** knowledge of *every* Godot built-in class, method, property, signal, enum,
constant, global function, singleton, and built-in Variant type — and a **robust, automated**
process to re-sync on **every Godot release**. This note specifies the data sources, the
schema, what the dump is missing, and the codifiable "rule files" for the sync pipeline.

**TL;DR recommendation.** Ingest Godot's machine-readable **`extension_api.json`** (the
`--dump-extension-api` output) as the single source of truth for the *engine/native* API —
the **same artifact** godot-rust/`gdext`, godot-cpp, and the C# bindings all generate from.
Bundle **one snapshot per supported Godot minor version**, pulled from
`godotengine/godot-cpp`'s `godot-<x.y>-stable` tags. For **hover/doc text**, ingest Godot's
per-class **doc XML** (`doc/classes/*.xml` + `modules/*/doc_classes/*.xml`), which is strictly
more complete than `--with-docs` (it has annotations, per-constant prose, and tutorials). The
dump is missing the **entire GDScript language layer** (keywords, annotations, `preload`/`load`/
`range`/`len`, the `@GDScript` pseudo-class) and **project-local symbols** (autoloads,
`class_name` globals) — we must supply those ourselves. Detect new releases via the GitHub
Releases API; diff two dumps to produce an API-delta changelog.

---

## 1. `extension_api.json` — what it is and its exact structure

`extension_api.json` is a machine-readable, complete description of the Godot engine's public
API (every built-in Variant type, every `Object`-derived class, all global enums/constants,
utility functions, singletons, and native structs), produced by introspecting the engine's
`ClassDB`. Its purpose is **automatic language-binding generation** (C++, C#, Rust, D, Swift…),
including for engine forks. It is paired with — but distinct from — `gdextension_interface.h`,
the low-level C ABI function table.

- Purpose & overview: <https://docs.godotengine.org/en/latest/tutorials/scripting/gdextension/gdextension_interface_json_file.html>,
  <https://docs.godotengine.org/en/stable/tutorials/scripting/gdextension/what_is_gdextension.html>

### 1.1 Verified top-level keys

Confirmed against the real file shipped in `godot-cpp@master` (Godot Engine **v4.7.stable**,
single precision, **≈6.7 MB**, 6,965,055 bytes):

```jsonc
{
  "header":                       { /* version + precision */ },
  "builtin_class_sizes":          [ /* 4 build configs */ ],
  "builtin_class_member_offsets": [ /* 4 build configs — NOT in the original spec */ ],
  "global_constants":             [ /* 11 in 4.7 — NOT always empty */ ],
  "global_enums":                 [ /* 22 */ ],
  "utility_functions":            [ /* 114 — @GlobalScope funcs */ ],
  "builtin_classes":              [ /* 38 — the Variant types */ ],
  "classes":                      [ /* 1036 — engine Object-derived classes */ ],
  "singletons":                   [ /* 41 */ ],
  "native_structures":            [ /* 14 */ ]
}
```

Two corrections vs. the naive spec: (a) there is an **extra** top-level key
`builtin_class_member_offsets` (sibling of `builtin_class_sizes`); (b) `global_constants`,
`global_enums`, `utility_functions` appear in the file *before* `builtin_classes`/`classes`.
Both confirmed from the actual file.

- Real file: <https://raw.githubusercontent.com/godotengine/godot-cpp/master/gdextension/extension_api.json>

### 1.2 Section-by-section (field names verbatim)

**`header`** (object) — the authoritative version block:
```json
{ "version_major": 4, "version_minor": 7, "version_patch": 0,
  "version_status": "stable", "version_build": "official",
  "version_full_name": "Godot Engine v4.7.stable.official",
  "precision": "single" }
```
`version_status` ∈ `"stable" | "beta" | "rc" | "dev"`. `precision` ∈ `"single" | "double"`.

**`builtin_class_sizes`** — array of 4 objects, one per build configuration. The four
`build_configuration` values are **`float_32`, `float_64`, `double_32`, `double_64`** (the
real-type × pointer-width matrix). Each: `{ "build_configuration": "...", "sizes": [{ "name": "Vector2", "size": 8 }, ...] }`.

**`builtin_class_member_offsets`** — same 4 configs; per-member byte offsets, e.g.
`Vector2 → [{ "member":"x","offset":0,"meta":"float" }, { "member":"y","offset":4,"meta":"float" }]`.

**`builtin_classes`** — the 38 Variant types (`Vector2`, `String`, `Dictionary`, …). Keys for
`Vector2`: `name, indexing_return_type, is_keyed, members, constants, enums, operators, methods, constructors, has_destructor`.
- `members`: `[{ "name":"x", "type":"float" }, ...]`
- `constants`: `[{ "name":"ZERO", "type":"Vector2", "value":"Vector2(0, 0)" }]` (note `value` is a string)
- `operators`: `[{ "name":"==", "right_type":"Variant", "return_type":"bool" }]` (`right_type` absent for unary) — **operators ARE present for Variant builtins**
- `constructors`: `[{ "index":0, "arguments":[...] }, ...]`
- `methods`: `[{ "name":"angle", "return_type":"float", "is_vararg":false, "is_const":true, "is_static":false, "hash":466405837, "arguments":[...] }]`
- `enums`: builtin-class enums have **no** `is_bitfield` field

**`classes`** — the 1036 engine `Object`-derived classes. Keys for `Node`:
`name, is_refcounted, is_instantiable, inherits, api_type, constants, enums, methods, signals, properties`.
- `api_type` ∈ **`"core" | "editor"`** only.
- `methods[]`: `name, is_const, is_vararg, is_static, is_virtual, hash, return_value, arguments`.
  Virtual methods add **`is_required`** (Godot 4.4+) and may omit `hash`.
  ```json
  { "name":"get_child_count", "is_const":true, "is_vararg":false, "is_static":false,
    "is_virtual":false, "hash":894402480,
    "return_value": { "type":"int", "meta":"int32" },
    "arguments": [ { "name":"include_internal", "type":"bool", "default_value":"false" } ] }
  ```
- `arguments[]`: `name`, `type`, optional `meta` (e.g. `int32`/`int64`/`float`), optional
  `default_value` (**always a JSON string**, e.g. `"false"`, `"1.0"`, `"Vector2(0, 0)"`).
- `return_value`: `{ "type", "meta?" }`.
- `properties[]`: `{ "type", "name", "setter", "getter", "index?" }` (`index` for indexed accessors).
- `signals[]`: `{ "name", "arguments?" }`.
- `constants[]`: `{ "name":"NOTIFICATION_ENTER_TREE", "value":10 }`.
- `enums[]`: class enums **do** carry `is_bitfield`: `{ "name":"ProcessMode", "is_bitfield":false, "values":[...] }`.

**`singletons`** — `{ "name":"Performance", "type":"Performance" }`. `type` currently mirrors
`name` and is treated as redundant by godot-rust.

**`global_constants`** — non-empty in 4.7 (11 entries): `{ "name":"UINT8_MAX", "value":255, "is_bitfield":false }`.

**`global_enums`** — `{ "name":"Side", "is_bitfield":false, "values":[{ "name":"SIDE_LEFT","value":0 }, ...] }`.

**`utility_functions`** — `{ "name":"sin", "return_type":"float", "category":"math", "is_vararg":false, "hash":2140049587, "arguments":[{ "name":"angle_rad","type":"float" }] }`.
`category` ∈ `"math" | "general"`. These objects do **not** carry `is_const`/`is_static`/`is_virtual`.

**`native_structures`** — `{ "name":"AudioFrame", "format":"float left;float right" }` (single
semicolon-delimited C-struct-field string).

### 1.3 How it is generated

Read verbatim from Godot's own `main/main.cpp@master` help strings (authoritative):

- **`--dump-extension-api`** — *"Generate a JSON dump of the Godot API for GDExtension bindings
  named `extension_api.json` in the current folder."*
- **`--dump-extension-api-with-docs`** — *"…like the previous option, but including documentation."*
- **`--dump-gdextension-interface`** — *"Generate a GDExtension header file `gdextension_interface.h`
  in the current folder."* (The C ABI header — a *different* file from `extension_api.json`.)
- **`--dump-gdextension-interface-json`** — newer (PR #107845); the C interface *as JSON*
  (`gdextension_interface.json`, yet another distinct file).

All require an **editor build**. Typical invocation (headless = dummy drivers, no window):
```sh
godot --headless --dump-extension-api --dump-gdextension-interface
```

- `main.cpp` help strings: <https://raw.githubusercontent.com/godotengine/godot/master/main/main.cpp>
- CLI reference (lists both flags): <https://docs.godotengine.org/en/stable/tutorials/editor/command_line_tutorial.html>

**Version availability.** GDExtension and `--dump-extension-api`/`--dump-gdextension-interface`
exist since **Godot 4.0** (GDExtension introduced by PR #49744). The **`--with-docs`** variant
was added in **Godot 4.2** (PR #82331; follow-up #83318 renamed the `documentation` key →
`description` and added `brief_description`).
- <https://github.com/godotengine/godot/pull/49744>, <https://github.com/godotengine/godot/pull/82331>

**File sizes.** No-docs dump ≈ **5.5–7 MB** (4.2 ≈ 5.5 MB; 4.7 measured 6.7 MB);
`--with-docs` ≈ **9 MB** (4.2: 9,121,481 bytes, +65%). gzip ≈ 400 KB / 1.2 MB.

### 1.4 Same data as C#/GDExtension bindings? Yes — and how godot-rust parses it

`extension_api.json` is the single source of truth shared by **all** GDExtension language
bindings, generated from `ClassDB` introspection — the same registry the .NET/C#
`BindingsGenerator` reads. Out-of-tree C# generators (raulsntos/`godot-dotnet`,
gilzoide/`godot-csharp-gdextension-bindgen`) consume it directly.

**godot-rust (`gdext`) is the model to copy.** Its `godot-codegen` crate parses the JSON in a
**two-stage pipeline**:

1. **JSON models** — `godot-codegen/src/models/api_json.rs` deserializes (with **`nanoserde::DeJson`**,
   not serde) into 1:1 `Json*` structs. Entry point:
   ```rust
   #[derive(DeJson)]
   pub struct JsonExtensionApi {
       pub header: JsonHeader,
       pub builtin_class_sizes: Vec<JsonBuiltinSizes>,
       pub builtin_classes: Vec<JsonBuiltinClass>,
       pub classes: Vec<JsonClass>,
       pub global_enums: Vec<JsonEnum>,
       pub utility_functions: Vec<JsonUtilityFunction>,
       pub native_structures: Vec<JsonNativeStructure>,
       pub singletons: Vec<JsonSingleton>,
   }
   ```
   Notable: it deliberately omits `global_constants`/`builtin_class_member_offsets` and some
   builtin sub-fields (nanoserde tolerates unknown JSON keys); `type` is `#[nserde(rename = "type")]`
   to dodge the Rust keyword; `JsonClassMethod.is_required` is `#[cfg(since_api = "4.4")]`.
2. **Domain mapping** — `godot-codegen/src/models/domain_mapping.rs` lowers each `Json*` into a
   cleaner domain model (`ExtensionApi`, `Class`, `ClassMethod`, `FnParam`, `Enum`, …) that the
   generators turn into Rust glue.

- <https://github.com/godot-rust/gdext/blob/master/godot-codegen/src/models/api_json.rs>
- <https://github.com/godot-rust/gdext/blob/master/godot-codegen/src/models/domain_mapping.rs>

> **Design lesson:** mirror this exact two-stage shape — a permissive raw-JSON deserialization
> layer (forward-compatible with new keys), then a normalized internal API model the analyzer's
> resolver queries. Don't bind the resolver directly to the JSON shape.

---

## 2. What the dump does — and does **NOT** — contain

`extension_api.json` is an **engine/native (ClassDB + Variant) dump only**. It is **not** a
GDScript-language description.

### 2.1 Present (confirmed yes)

| Item | In dump? | Notes |
|---|---|---|
| Argument **names** | ✅ | `arguments[].name` |
| Argument **types** | ✅ | `arguments[].type` (+ `meta` width) |
| **Default argument values** | ✅ | `arguments[].default_value` (string-encoded) |
| **Return types** | ✅ | class methods: `return_value.{type,meta}`; utility: flat `return_type` (omitted for void) |
| **Enum integer values** | ✅ | `enums[].values[].value` |
| **Bitfield flag** | ✅ | `is_bitfield` on global/class enums (NOT on builtin-class enums) |
| `is_vararg` / `is_const` / `is_static` / `is_virtual` | ✅ | on methods (utility funcs only have `is_vararg`) |
| Method **`hash`** | ✅ | per non-virtual method + per utility func — *the signature-change signal* |
| `@GlobalScope` utility functions | ✅ | as `utility_functions` (`print`, `str`, `typeof`, `min/max`, `sin/cos`, `weakref`, `is_instance_valid`, …) |
| Operators on Variant builtins | ✅ | `builtin_classes[].operators` (`Vector2 + Vector2`, etc.) |
| Engine singletons | ✅ | `singletons` array (`Input`, `OS`, `Engine`, …) |

- Field confirmations via godot-cpp's `binding_generator.py` (canonical consumer):
  <https://github.com/godotengine/godot-cpp/blob/master/binding_generator.py>

### 2.2 NOT present — the GDScript language layer (we must supply this)

- **GDScript-only built-in functions** — `preload`, `load`, `range`, `len`, `char`, `ord`,
  `convert`, `dict_to_inst`, `inst_to_dict`, `get_stack`, `print_debug`, `print_stack`,
  `type_exists`, `is_instance_of`, `Color8`, `assert`. **`preload`/`load` appear nowhere in the
  dump.** These are the entire `@GDScript` `<methods>` set.
- **GDScript annotations** — `@export*` (and all the `_range/_enum/_flags/_file/_dir/_group/…`
  variants), `@onready`, `@tool`, `@rpc`, `@icon`, `@warning_ignore[_start/_restore]`,
  `@abstract`, `@static_unload`, `@export_category/group/subgroup`. **No annotation section
  exists in the dump.**
- **GDScript keywords** — `func`, `var`, `const`, `static`, `extends`, `class_name`, `class`,
  `enum`, `signal`, `match`/`when`, `await`, `if/elif/else`, `for/in`, `while`, `return`,
  `pass`, `break`, `continue`, `and/or/not`, `is`, `as`, `in`, `self`, `super`, `true/false/null`,
  `void`, `breakpoint`. (Expected — the dump is API metadata, not a grammar.)

### 2.3 The `@GlobalScope` / `@GDScript` pseudo-classes — **doc-only, NOT in `classes`**

This is load-bearing: **`@GlobalScope` and `@GDScript` are documentation-only pseudo-classes**.
They exist as XML (`doc/classes/@GlobalScope.xml`, `modules/gdscript/doc_classes/@GDScript.xml`)
and render to the docs site, but are **not registered in `ClassDB`**, so they are **never** entries
in `extension_api.json`'s `classes` array.

- `classes` is a `ClassDB` iteration; the pseudo-classes are synthetic doc namespaces.
- `@GlobalScope`'s *content* is split across real dump sections: its methods → `utility_functions`;
  its constants/enums → `global_constants` + `global_enums`; its singleton members → `singletons`.
  So the *information* is present, just not under a class named `@GlobalScope`.
- `@GDScript`'s content (`preload`/`load`/`range`/`len` + annotations) maps to **nothing** in the
  dump — it is exactly the language layer the dump omits.
- Byte-level confirmation procedure: generate locally and grep — `"@GlobalScope"`/`"@GDScript"`
  will **not** appear as `classes[].name`.
- `@GDScript` reference: <https://docs.godotengine.org/en/stable/classes/class_@gdscript.html>
- `@GlobalScope` reference: <https://docs.godotengine.org/en/stable/classes/class_@globalscope.html>

### 2.4 Documentation in the dump

By default **no** descriptions. `--dump-extension-api-with-docs` adds a `description` field on
classes/builtin_classes/methods/properties/signals/constants/enums, plus `brief_description` on
(builtin) classes. The text is **raw BBCode lifted verbatim from the doc XML** (confirmed by
issue #86080, where wrong type names leak through unchanged).
- <https://github.com/godotengine/godot/pull/82331>, <https://github.com/godotengine/godot/issues/86080>

### 2.5 Full list of what we must supply ourselves

1. **GDScript keywords** (from the tokenizer / GDScript reference).
2. **GDScript annotations** list + docs (only in `@GDScript.xml`).
3. **GDScript built-in functions** (`preload`, `load`, `range`, `len`, `char`, `ord`, `convert`,
   `dict_to_inst`, `inst_to_dict`, `get_stack`, `print_debug`, `print_stack`, `type_exists`,
   `is_instance_of`, `Color8`, `assert`).
4. **GDScript type grammar / inference rules** — `Variant`/`void`, typed `Array[T]`/`Dictionary[K,V]`,
   `is`/`as` semantics, lambdas, `await` on signals/coroutines.
5. **Project-local symbols** — **autoloads** (`project.godot [autoload]`), **`class_name` globals**
   (the global class registry / `.godot/global_script_class_cache.cfg`), `[global_group]` groups,
   `[input]` action-name strings. *(See §6 + companion note on project ingestion.)*

---

## 3. Documentation XML vs. `--with-docs` (hover/doc source)

**Recommendation: use the doc XML as the primary hover source.** It is strictly more complete.

### 3.1 Where the XML lives

- **`doc/classes/*.xml`** — one file per engine class. **~812 files** on `master` (grows over 4.x;
  community refs cite ~829 at a later commit). `@GlobalScope.xml` lives here.
- **`modules/<module>/doc_classes/*.xml`** — **~261 files across 25 modules** (csg, gdscript,
  gltf, openxr, regex, websocket, …). **`@GDScript.xml` lives in `modules/gdscript/doc_classes/`**,
  not in `doc/classes/` — important for our fetch paths.
- Both use the identical format and are regenerated with `godot --doctool` (GDScript classes:
  `--doctool --gdscript-docs <project>`).

### 3.2 Structure (real `Node.xml`)

```xml
<class name="Node" inherits="Object" api_type="core"
       xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
       xsi:noNamespaceSchemaLocation="../class.xsd">
  <brief_description>Base class for all scene objects.</brief_description>
  <description> ... </description>
  <tutorials>
    <link title="Nodes and scenes">$DOCS_URL/getting_started/step_by_step/nodes_and_scenes.html</link>
  </tutorials>
  <methods>
    <method name="_process" qualifiers="virtual">
      <return type="void" />
      <param index="0" name="delta" type="float" />
      <description>Called on each idle frame ... [param delta] is the time between frames.</description>
    </method>
  </methods>
  <members>
    <member name="auto_translate_mode" type="int" setter="set_auto_translate_mode"
            getter="get_auto_translate_mode" enum="Node.AutoTranslateMode" default="0"> ... </member>
  </members>
  <signals> <signal name="tree_entered"><description> ... [constant NOTIFICATION_ENTER_TREE] ...</description></signal> </signals>
  <constants> <constant name="PROCESS_MODE_INHERIT" value="0" enum="ProcessMode"> ... </constant> </constants>
</class>
```

`@GDScript.xml` additionally has `<annotations><annotation name="@export">…</annotation></annotations>`
— **`<annotation>` elements exist in no other XML and are absent from `extension_api.json` entirely.**
All human text is **BBCode** (`[param x]`, `[ClassName]`, `[method foo]`, `[member x]`,
`[constant X]`, `[b]`, `[codeblocks]/[gdscript]/[csharp]`).

### 3.3 Why XML wins

| Capability | doc XML | `--with-docs` JSON |
|---|---|---|
| Class/method/member/signal descriptions | ✅ | ✅ (same BBCode strings) |
| Per-constant / per-enum-value prose | ✅ | ⚠️ flattened for codegen |
| `<tutorials>` links | ✅ | ❌ |
| **GDScript `@annotations`** | ✅ (`@GDScript.xml`) | ❌ not in schema |
| `@GDScript` builtins (`preload`/`len`/`range`) | ✅ | ❌ |
| Multi-language `[codeblocks]` examples | ✅ | ✅ (embedded) |

The official **docs website is generated from the XML** via `doc/tools/make_rst.py`
(*"makes RST files from the XML class reference for use with the online docs"*), and the
in-editor F1 help is XML-driven too. The `--with-docs` JSON is a *convenience* for binding
generators (one file vs. ~800), not the canonical doc source.
- <https://raw.githubusercontent.com/godotengine/godot/master/doc/classes/Node.xml>
- <https://raw.githubusercontent.com/godotengine/godot/master/doc/tools/make_rst.py>
- <https://github.com/godotengine/godot/blob/master/modules/gdscript/doc_classes/GDScript.xml>
- Class-reference contributing (XML is canonical): <https://docs.godotengine.org/en/stable/engine_details/class_reference/index.html>

**Mandatory post-processing: BBCode → Markdown** for LSP `textDocument/hover` (`MarkupContent`).
Map `[b]→**`, `[i]→*`, `[code]→`backtick``, `[codeblocks]/[gdscript]/[csharp]→fenced
```gdscript```, cross-refs (`[ClassName]`, `[method X]`, `[member X]`, `[constant X]`,
`[signal X]`, `[param x]`, `[enum X]`, `[annotation @x]`) → backticked names or links into our
symbol index, `[url=…]text[/url]→Markdown link`, and resolve `$DOCS_URL →
https://docs.godotengine.org/en/<version>`. Strip unhandled tags so no literal `[...]` reaches
the hover. (Model on `make_rst.py`'s `format_text_block()`.)

> **Practical hybrid:** ingest `--with-docs` JSON for engine-class descriptions (one parse) **and**
> ingest `@GDScript.xml` + `@GlobalScope.xml` for annotations / GDScript builtins / globals (which
> the JSON lacks). Uniform XML is simpler and strictly more complete if we only want one pipeline.

---

## 4. Versioning & breaking changes

### 4.1 Scheme

Godot is **semver-*ish* but not strict SemVer**: `MAJOR.MINOR.PATCH` where **MINOR = feature
release** (the API-extending/breaking unit) and **PATCH = maintenance/bugfix** (meant to stay
compatible). So `4.1, 4.2, 4.3, 4.4, 4.5` are feature releases; `4.5.1, 4.5.2` are maintenance.
The authoritative version is the `header` block (§1.2).
- <https://godotengine.org/article/maintenance-release-godot-4-5-2/>

### 4.2 ABI compatibility policy (method hashes + compatibility methods)

**Rule:** *extensions built for an older Godot work in newer minors, not vice-versa*
(`runtime version ≥ API version`). The one deliberate exception was the **4.0 → 4.1 break**
(entry-point signature changed; `compatibility_minimum = 4.1` required).
- <https://docs.godotengine.org/en/4.4/tutorials/scripting/gdextension/what_is_gdextension.html>,
  <https://godotengine.org/article/godot-4-1-is-here/>

The **method-hash / compatibility-method** mechanism (PR #76446, lands in 4.1) is the key signal
for our sync diff:
- Every method's `hash` is computed from its **full signature**. **Signature changes ⇒ hash changes.**
- Godot keeps **compatibility methods** (`bind_compatibility_method`) registered under old hashes so
  old binaries keep working; `godot --validate-extension-api <old.json>` reports hash mismatches /
  missing compat funcs (non-zero exit = CI-friendly).
- Virtual-method compat came later (PR #100674, **4.4**); the `is_required` field appears 4.4+.
- A `hash` is only meaningful within a **(version, precision)** tuple — double-precision builds
  produce different hashes (issue #86346).
- <https://github.com/godotengine/godot/pull/76446>, <https://github.com/godotengine/godot/pull/100674>,
  <https://github.com/godotengine/godot/issues/86346>

### 4.3 Where breaks are tracked

- **`CHANGELOG.md`** (per release branch) — categorized by subsystem (2D/3D/Core/Editor/
  GDExtension/GDScript/…), prose bullets with PR links. Human-readable, **not** a machine-parseable
  per-method manifest. <https://github.com/godotengine/godot/blob/master/CHANGELOG.md>
- **Interactive Changelog** — generates `CHANGELOG.md`/blog notes; public artifact is a website,
  not a stable API. <https://godotengine.github.io/godot-interactive-changelog/>
- **Release-notes articles** (prose): 4.1 <https://godotengine.org/article/godot-4-1-is-here/>,
  4.5 <https://godotengine.org/releases/4.5/>; GitHub Releases per tag.

**Conclusion:** the only deterministic, fully machine-readable break signal is **diffing two
`extension_api.json` dumps**, using per-method `hash` changes as the signature-change signal.
Prior art: **`mhilbrunner/gdapi-diff`** ("Compares the API JSON dump of different Godot Engine
versions"). <https://github.com/mhilbrunner/gdapi-diff>

### 4.4 How godot-rust handles multiple versions (the model)

- **`api-4-{minor}`** / **`api-4-{minor}-{patch}`** mutually-exclusive Cargo features select the
  target API; **default = current minor at patch 0**.
- **`api-custom`** (build against a local Godot binary, env `GDRUST_GODOT_BIN`) and
  **`api-custom-json`** (env `GDRUST_GODOT_API_JSON` → a hand-supplied dump).
- **Prebuilt per-version dumps** live in **`godot-rust/godot4-prebuilt`** — one branch per Godot
  version, each with `res/extension_api.json` (+ `res/meta.json`).
- Codegen emits **`#[cfg(since_api = "4.x")]` / `#[cfg(before_api = "4.x")]`** gates so symbols only
  exist when the targeted API supports them — *the pattern our analyzer should mirror per symbol*.
- <https://godot-rust.github.io/book/toolchain/godot-version.html>,
  <https://godot-rust.github.io/book/toolchain/compatibility.html>,
  <https://github.com/godot-rust/gdext/issues/1006>

---

## 5. Where to obtain per-version `extension_api.json`

**All URLs verified to resolve (June 2026); reported version = the `header` of the returned JSON.**

### 5.1 Primary: `godotengine/godot-cpp` (`gdextension/extension_api.json`)

- **Tag (recommended, exact-patch reproducible):**
  `https://raw.githubusercontent.com/godotengine/godot-cpp/godot-<VER>-stable/gdextension/extension_api.json`
- **Branch (latest patch of a minor line):**
  `https://raw.githubusercontent.com/godotengine/godot-cpp/<x.y>/gdextension/extension_api.json`
- Companion interface header: same dir, `gdextension_interface.h`.
- Verified: `godot-4.3-stable` tag → 4.3.0; branch `4.0` → 4.0.4; `4.1` → 4.1.4; `4.5` → 4.5.0;
  `master` → 4.7.0. **Branches track the latest patch of the line; use tags to pin a patch.**
- <https://github.com/godotengine/godot-cpp/tree/master/gdextension>

### 5.2 Secondary / cross-check: `godot-rust/godot4-prebuilt`

- `https://raw.githubusercontent.com/godot-rust/godot4-prebuilt/<BRANCH>/res/extension_api.json`
- Branch = the Godot version string (`4.1.4, 4.2, 4.2.1, 4.2.2, 4.3, 4.4, 4.4.1, 4.5, 4.5.1, 4.5.2, 4.6…, 4.7`).
  Earliest prebuilt is **`4.1.4`** (use godot-cpp's `godot-4.0-stable` for 4.0). No
  `gdextension_interface.h` alongside (kept separately). <https://github.com/godot-rust/godot4-prebuilt>

### 5.3 Godot release assets — **do NOT** ship the JSON

`godotengine/godot/releases` (stable only) and `godotengine/godot-builds` (dev/beta/rc) attach
editor/template binaries, `.tpz`, checksums — **never** `extension_api.json` or
`gdextension_interface.h`. Confirmed for 4.7-stable.

### 5.4 Generate it yourself (any version / forks)

Download the editor binary (`Godot_v<VER>-stable_<platform>.zip` from GitHub releases or
`https://downloads.godotengine.org/`), then:
```sh
godot --headless --dump-extension-api --dump-gdextension-interface
```
For docs: add `--dump-extension-api-with-docs`. Source mirror archive:
`https://godotengine.org/download/archive/<VER>-stable/`.

### 5.5 Version → URL table

| Godot | godot-cpp tag URL (pin)                                                                                   | godot4-prebuilt branch                         |
|-------|-----------------------------------------------------------------------------------------------------------|------------------------------------------------|
| 4.0   | `…/godot-cpp/godot-4.0-stable/gdextension/extension_api.json`                                              | n/a (earliest is 4.1.4) → use godot-cpp        |
| 4.1   | `…/godot-cpp/godot-4.1-stable/gdextension/extension_api.json`                                              | `…/godot4-prebuilt/4.1.4/res/extension_api.json` |
| 4.2   | `…/godot-cpp/godot-4.2-stable/gdextension/extension_api.json`                                              | `…/godot4-prebuilt/4.2/res/extension_api.json` (+4.2.1/4.2.2) |
| 4.3   | `…/godot-cpp/godot-4.3-stable/gdextension/extension_api.json` ✅ 4.3.0                                     | `…/godot4-prebuilt/4.3/res/extension_api.json` ✅ 4.3.0 |
| 4.4   | `…/godot-cpp/godot-4.4-stable/…` (+`godot-4.4.1-stable`)                                                   | `…/godot4-prebuilt/4.4/res/…` (+4.4.1)         |
| 4.5   | `…/godot-cpp/godot-4.5-stable/…`  (branch `4.5` ✅ 4.5.0)                                                  | `…/godot4-prebuilt/4.5/res/…` (+4.5.1/4.5.2)   |
| 4.6   | *(tag may trail; use branch/master or generate)*                                                          | `…/godot4-prebuilt/4.6/res/…` (+4.6.1/4.6.2)   |
| 4.7   | `…/godot-cpp/master/gdextension/extension_api.json` ✅ 4.7.0                                               | `…/godot4-prebuilt/4.7/res/…`                  |

(Base for godot-cpp rows: `https://raw.githubusercontent.com`.) **Caveat:** godot-cpp tags trail
Godot stable slightly; if a `godot-<x.y>-stable` tag doesn't exist yet, fall back to `master` or
generate from a binary.

---

## 6. Multi-version support & project version detection

### 6.1 Detect the project's targeted version from `project.godot`

`project.godot` is a Godot `ConfigFile` (INI-like, typed values). Detection signals, in order:

1. **`[application] config/features = PackedStringArray("4.3", "Forward Plus")`** — the
   **authoritative** signal. The **version entry is the `major.minor` string** (e.g. `"4.3"`);
   other entries are renderer tags. **Replicate the engine's own heuristic** (from
   `core/config/project_settings.cpp`): *scan the array for the entry matching `\d+\.\d+`* — do
   **not** assume index 0, and don't assume ≥2 elements (a project may have only `"4.6"`). The
   editor **rewrites** this on save from its own `GODOT_VERSION_BRANCH`, so it reflects "last
   editor that saved the project" — the best available target proxy.
2. **`config_version=N`** (top line) — coarse era marker only: **`5` = Godot 4.x**, `4` = 3.x. Not
   a minor-version signal. (`CONFIG_VERSION = 5` in `core/config/project_settings.h`.)
3. **`.gdextension` `compatibility_minimum`** (4.1+; must be `major.minor`, not patch — issue
   #80590) — a **floor**, not the target. `compatibility_maximum` added 4.3.

Real example:
```ini
config_version=5

[application]
config/name="GDShell"
run/main_scene="res://addons/gdshell/demo/demo.tscn"
config/features=PackedStringArray("4.0", "Forward Plus")

[autoload]
GDShell="*res://addons/gdshell/scripts/gdshell_main.gd"
```

- `[autoload]`: key = global singleton name; value = `res://` path, **`*` prefix = enabled global
  singleton** (strip with `trim_prefix("*")`). Autoloads are **project-local globals absent from
  `extension_api.json`** — the analyzer must parse them and type each from its target script.
- Engine source (version parse + config_version + features write):
  <https://raw.githubusercontent.com/godotengine/godot/master/core/config/project_settings.cpp>,
  <https://raw.githubusercontent.com/godotengine/godot/master/core/config/project_settings.h>
- Autoload docs: <https://docs.godotengine.org/en/stable/tutorials/scripting/singletons_autoload.html>
- `.gdextension` `compatibility_minimum`: <https://docs.godotengine.org/en/4.4/tutorials/scripting/gdextension/gdextension_file.html>

### 6.2 How comparable tools do it (and why we differ)

- **Godot's built-in LSP** — version-locked to the running editor (the editor *is* the version).
- **`godot-tools` VS Code** — bundles **no** API; connects over TCP/JSON-RPC to the **running
  editor's** LSP (default port **6005**; client tries 6005/6008), so it inherits the editor's
  version. Requires a running editor (or 4.2+ headless LSP).
- **godot-rust** — bundles per-version dumps, selects via `api-4-x`, defaults to current minor,
  gates symbols with `since_api`/`before_api`.

A **standalone** analyzer has no running editor, so it must own its API data and select by project
version — the godot-rust model, applied to GDScript symbols.

### 6.3 Recommended versioning model

1. **Bundle one API snapshot per supported Godot minor** (e.g. 4.0…4.7), keyed by `major.minor`.
   Each snapshot = that version's `extension_api.json` (engine API) + the GDScript layer
   (`@GDScript`/`@GlobalScope` from XML) + keyword/annotation tables. Pull from godot-cpp
   `godot-<x.y>-stable` tags.
2. **Detect** the version from `config/features` (§6.1 heuristic).
3. **Map** detected → nearest bundled snapshot (exact match preferred; otherwise snap **down** to
   the highest bundled minor ≤ detected — additive API is a superset, but watch removals/renames).
4. **Fall back to the newest bundled snapshot** when `config/features` is missing/malformed (what
   godot-rust does).
5. **Layer project-local symbols on top** — autoloads, `class_name` globals, `[global_group]`,
   `[input]` actions — merged into the resolution scope.
6. **Allow an explicit override** (analyzer setting) to force a bundled version, for stale
   `config/features` or cross-target validation — mirroring `api-4-x` / `api-custom`.
- <https://godot-rust.github.io/book/toolchain/godot-version.html>,
  <https://raw.githubusercontent.com/godotengine/godot-vscode-plugin/master/README.md>

---

## 7. The SYNC PROCESS — codifiable "rule files"

A two-actor pipeline: a **CI bot** (detect + fetch + diff + draft PR) and a **maintainer**
(review + merge + release). Codify the constants below in a checked-in `godot-sync.toml`-style
rule file consumed by the CI job.

### 7.1 Rule-file constants (checked into the repo)

```toml
# godot-sync rules — single source of truth for the sync job
detect.releases_api   = "https://api.github.com/repos/godotengine/godot/releases/latest"
detect.tags_api       = "https://api.github.com/repos/godotengine/godot/tags"
detect.prerelease_api = "https://api.github.com/repos/godotengine/godot-builds/releases" # dev/beta/rc (optional)
detect.tag_regex      = '^(\d+)\.(\d+)(?:\.(\d+))?-stable$'   # ignore -dev/-beta/-rc unless opted in

fetch.api_json.primary   = "https://raw.githubusercontent.com/godotengine/godot-cpp/godot-{ver}-stable/gdextension/extension_api.json"
fetch.api_json.branch    = "https://raw.githubusercontent.com/godotengine/godot-cpp/{minor}/gdextension/extension_api.json"
fetch.api_json.secondary = "https://raw.githubusercontent.com/godot-rust/godot4-prebuilt/{ver}/res/extension_api.json"
fetch.doc_xml.repo       = "https://github.com/godotengine/godot"          # tag: {ver}-stable
fetch.doc_xml.classes    = "doc/classes/*.xml"
fetch.doc_xml.gdscript   = "modules/gdscript/doc_classes/@GDScript.xml"
fetch.doc_xml.globalscope= "doc/classes/@GlobalScope.xml"
fetch.generate_fallback  = "godot --headless --dump-extension-api --dump-extension-api-with-docs --dump-gdextension-interface"

supported_minors = ["4.0","4.1","4.2","4.3","4.4","4.5","4.6","4.7"]
default_minor    = "4.7"   # newest bundled; fallback when project version unknown
hash_scope       = "single" # method hashes valid only per (version, precision)
```

### 7.2 Pipeline (step list)

**(a) DETECT — CI bot, scheduled (e.g. daily) + on demand**
1. `GET detect.releases_api`; also `GET detect.tags_api`. Parse `tag_name` against `detect.tag_regex`.
2. If a new `{ver}` not in `supported_minors`/bundled set → open a tracking issue, proceed.

**(b) OBTAIN the new `extension_api.json` — CI bot**
3. Try `fetch.api_json.primary` (godot-cpp tag). If the tag doesn't exist yet (godot-cpp trails),
   try `fetch.api_json.secondary` (godot4-prebuilt), then `fetch.api_json.branch`.
4. Last resort: download the editor binary and run `fetch.generate_fallback` (also yields the
   `--with-docs` JSON + interface header).
5. Fetch the matching **doc XML** at tag `{ver}-stable` (`doc/classes/*.xml` +
   `modules/gdscript/doc_classes/@GDScript.xml` + `@GlobalScope.xml`).
6. Record provenance (source URL, `header.version_full_name`, `header.precision`, fetch date) in a
   `meta.json` beside the snapshot.

**(c) DIFF against the current snapshot — CI bot**
7. Structural JSON diff (model on `mhilbrunner/gdapi-diff`): per **class** added/removed; per
   **method/property/signal/enum/constant** added/removed; per-method **`hash` change ⇒ signature
   change** (compare within same `precision`). Classify into Added / Removed / Changed-signature /
   Deprecated.
8. Emit a human-readable **API-delta changelog** (`deltas/<old>__<new>.md`).

**(d) REGENERATE the internal API model — CI bot**
9. Re-run our codegen (raw-JSON → normalized model, §1.4 two-stage), tagging each symbol with
   `since_api`/`before_api` availability. Regenerate the doc index (BBCode→Markdown, §3.3),
   including the GDScript layer (annotations/builtins) from XML.

**(e) TEST — CI bot**
10. Run the analyzer test suite against the new snapshot (resolution/hover/completion fixtures).
    Add golden tests asserting key symbols exist (e.g. `Node.add_child`, `@export`, `preload`).
    Fail the job on regressions.

**(f) SURFACE & SHIP — bot drafts, maintainer decides**
11. Bot opens a PR: new snapshot + regenerated model + `deltas/*.md` + test results.
12. **Maintainer** reviews the delta (esp. Removed/Changed-signature — potential consumer breakage),
    updates `supported_minors`/`default_minor` if adding a new minor, merges, tags a release.

### 7.3 What each actor does (summary)

- **CI bot:** detect releases; fetch/generate the dump + doc XML; diff; regenerate model + docs;
  run tests; open a PR with the delta changelog. Fully automated, idempotent, re-runnable.
- **Maintainer:** review the API-delta PR (breakage triage), decide whether to add a new minor to
  the bundled set + bump `default_minor`, merge, cut a release. Owns the judgment calls
  (deprecations, renames, dropping EOL minors).

---

## 8. Decisions / open questions

- **Decision:** single source of truth for the engine API = **`extension_api.json`** (no-docs),
  bundled per minor from **godot-cpp `godot-<x.y>-stable` tags**; doc/hover from **doc XML** (+
  `@GDScript.xml`/`@GlobalScope.xml`); GDScript language layer hand-maintained.
- **Decision:** mirror godot-rust's **two-stage parse** (permissive raw JSON → normalized model)
  and **`since_api`/`before_api`** per-symbol gating.
- **Decision:** detect target via `config/features` (engine heuristic), snap to nearest bundled
  minor, default to newest, allow override.
- **Open:** do we also track **dev/beta/rc** (godot-builds) for early-adopter feedback, or
  stable-only? (Default: stable-only; opt-in pre-release.)
- **Open:** ship `--with-docs` JSON *and* XML, or XML-only for docs? (Lean: XML-only pipeline —
  strictly more complete, one format.)
- **Open:** how to encode method `hash` per `(version, precision)` — bundle only single-precision,
  or both? (Default: single; revisit if double-precision support is requested.)

---

### Source index (all primary)

- extension_api.json purpose + interface JSON: <https://docs.godotengine.org/en/latest/tutorials/scripting/gdextension/gdextension_interface_json_file.html> · <https://docs.godotengine.org/en/stable/tutorials/scripting/gdextension/what_is_gdextension.html>
- Real file (4.7): <https://raw.githubusercontent.com/godotengine/godot-cpp/master/gdextension/extension_api.json>
- Dump commands (main.cpp): <https://raw.githubusercontent.com/godotengine/godot/master/main/main.cpp> · CLI ref: <https://docs.godotengine.org/en/stable/tutorials/editor/command_line_tutorial.html>
- `--with-docs` PR (4.2): <https://github.com/godotengine/godot/pull/82331> · bug: <https://github.com/godotengine/godot/issues/86080>
- GDExtension intro PR (4.0): <https://github.com/godotengine/godot/pull/49744>
- godot-rust codegen: <https://github.com/godot-rust/gdext/blob/master/godot-codegen/src/models/api_json.rs> · <https://github.com/godot-rust/gdext/blob/master/godot-codegen/src/models/domain_mapping.rs>
- godot-rust version selection / compat: <https://godot-rust.github.io/book/toolchain/godot-version.html> · <https://godot-rust.github.io/book/toolchain/compatibility.html> · <https://github.com/godot-rust/gdext/issues/1006>
- binding_generator.py (field confirmations): <https://github.com/godotengine/godot-cpp/blob/master/binding_generator.py>
- @GDScript / @GlobalScope docs: <https://docs.godotengine.org/en/stable/classes/class_@gdscript.html> · <https://docs.godotengine.org/en/stable/classes/class_@globalscope.html>
- Doc XML (Node, make_rst, @GDScript): <https://raw.githubusercontent.com/godotengine/godot/master/doc/classes/Node.xml> · <https://raw.githubusercontent.com/godotengine/godot/master/doc/tools/make_rst.py> · <https://github.com/godotengine/godot/blob/master/modules/gdscript/doc_classes/GDScript.xml>
- Class-reference contributing (XML canonical): <https://docs.godotengine.org/en/stable/engine_details/class_reference/index.html>
- ABI compat (hashes/compat methods): <https://github.com/godotengine/godot/pull/76446> · virtual compat (4.4): <https://github.com/godotengine/godot/pull/100674> · precision/hash: <https://github.com/godotengine/godot/issues/86346>
- CHANGELOG / interactive changelog / release notes: <https://github.com/godotengine/godot/blob/master/CHANGELOG.md> · <https://godotengine.github.io/godot-interactive-changelog/> · <https://godotengine.org/article/godot-4-1-is-here/> · <https://godotengine.org/releases/4.5/>
- Diff prior art: <https://github.com/mhilbrunner/gdapi-diff>
- Per-version sources: <https://github.com/godotengine/godot-cpp/tree/master/gdextension> · <https://github.com/godot-rust/godot4-prebuilt> · releases (no JSON asset): <https://github.com/godotengine/godot/releases> · <https://github.com/godotengine/godot-builds> · download mirror: <https://downloads.godotengine.org/>
- project.godot detection: <https://raw.githubusercontent.com/godotengine/godot/master/core/config/project_settings.cpp> · <https://raw.githubusercontent.com/godotengine/godot/master/core/config/project_settings.h> · autoloads: <https://docs.godotengine.org/en/stable/tutorials/scripting/singletons_autoload.html> · `.gdextension`: <https://docs.godotengine.org/en/4.4/tutorials/scripting/gdextension/gdextension_file.html>
- godot-tools LSP coupling: <https://raw.githubusercontent.com/godotengine/godot-vscode-plugin/master/README.md>
