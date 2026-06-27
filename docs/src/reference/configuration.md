# Configuration

The analyzer reads its project model and its warning configuration from your **`project.godot`**
— the same file Godot uses. There is no separate analyzer config file; point a tool at a directory
containing `project.godot` (or pass it explicitly) and the settings below apply. See the full list
of codes in the [Warning Reference](./warnings.md).

## Where settings come from

| Setting | `project.godot` location | Effect |
|---|---|---|
| Master switch | `[debug]` → `gdscript/warnings/enable` | `false` silences **all** warnings. |
| Treat as errors | `[debug]` → `gdscript/warnings/treat_warnings_as_errors` | Escalates every `Warn` to `Error`. |
| Exclude addons | `[debug]` → `gdscript/warnings/exclude_addons` | Suppresses warnings under `res://addons/**`. |
| Per-code level | `[debug]` → `gdscript/warnings/<key>` | `0` = ignore, `1` = warn, `2` = error. |
| Engine version | `[application]` → `config/features` | Gates version-specific (`master`-only) codes. |

`<key>` is the code's lowercased name — e.g. `INTEGER_DIVISION` → `gdscript/warnings/integer_division`.

```ini
[application]
config/features=PackedStringArray("4.5")

[debug]
gdscript/warnings/enable=true
gdscript/warnings/treat_warnings_as_errors=false
gdscript/warnings/exclude_addons=true
gdscript/warnings/integer_division=2      ; promote to an error
gdscript/warnings/unused_parameter=0      ; silence
```

## Default levels: standalone vs project

- **With a `project.godot`** the analyzer follows **Godot's own defaults** (`default_warning_levels`):
  the type-strictness group (`UNSAFE_*`, `UNTYPED_DECLARATION`, …) is **ignored** by default, exactly
  like the engine. Your `project.godot` overrides win.
- **Standalone** (no `project.godot` — a single file, a quick CLI check) the analyzer runs **strict**:
  the opt-in type-strictness group is promoted to `Warn`, so you see the analyzer's full value without
  configuring anything.

The per-code *engine default* and the earliest applicable Godot version are listed for every code in
the [Warning Reference](./warnings.md).

## Inline suppression — `@warning_ignore`

Suppress a warning at the source, exactly like Godot:

```gdscript
@warning_ignore("integer_division")
var ticks := total / per_second        # the next statement only

@warning_ignore_start("unused_parameter")
func _process(delta):                  # a region …
    pass
@warning_ignore_restore("unused_parameter")   # … until restored (or end of file)
```

- `@warning_ignore("a", "b")` suppresses the listed codes over the **single following**
  statement/declaration.
- `@warning_ignore_start("a")` … `@warning_ignore_restore("a")` suppress a **region**; an
  unrestored `_start` runs to the end of the file.
- The argument is the setting key (the lowercased code name). Unknown names are currently ignored.

## Precedence

For a given warning, the resolved level is decided in this order: **master switch** → **per-code
level** (explicit override, else the engine/standalone default) → **treat-as-errors** (Warn → Error)
→ **scope** (`exclude_addons`) → **`@warning_ignore`** (overrides everything). The analyzer-native
diagnostics that have no engine setting key — `TYPE_MISMATCH`, `INVALID_NODE_PATH`,
`CYCLIC_INHERITANCE` — are always reported and are not gated.
