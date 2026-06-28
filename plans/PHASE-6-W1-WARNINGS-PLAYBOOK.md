# Phase 6 · Workstream 1 — The Full Godot Warning Set + Gating + Suppression (Playbook)

> Implementation playbook for completing the Godot warning surface to **1.0**: emit all gateable
> warnings with engine-matching codes/messages/levels, **gate** them from `project.godot`'s
> `debug/gdscript/warnings/*`, and honor `@warning_ignore[_start|_restore]`. Grounded against the
> *current* code (not the plan's sketch) and corrected where the two diverge.
>
> **Parents:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 1,
> [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md)
> §2.2 (the 48-warning set + `default_warning_levels[]`), §2.3 (gating + `@warning_ignore`),
> §1.10 (#34–36 the ignore annotations), [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2/§3 (the
> `gdscript-ide` contract + salsa durability).
>
> **Format bar:** the Phase-5 playbooks (`PHASE-5-{CLI,LSP,NAPI}-PLAYBOOK.md`).

---

## 0. Thesis

Most of this workstream's *engineering* is **not** writing 40 new checks — it's introducing the
**emit-then-gate seam** that the V1 plan claims Phase 2 already built but **the code does not have
yet**. Today every diagnostic is emitted with its **severity baked in** at the emit site
(`Context::emit(range, Severity::Warning, CODE, msg)` in `crates/gdscript-hir/src/infer.rs`), with a
bare `code: String` and no path to a per-code level, a project setting, or an `@warning_ignore`. The
1.0 job, in priority order:

1. **Introduce a typed `WarningCode` + a pure `gate()` function** so severity becomes a *resolved*
   property, not a hard-coded one — without invalidating `infer`/`item_tree` when a warning setting
   changes (the salsa-cacheability constraint, §6).
2. **Parse `debug/gdscript/warnings/*`** from the already-loaded `project.godot` (extend
   `crates/gdscript-hir/src/project.rs`, which already parses `[autoload]` and `[application]`).
3. **Build the `@warning_ignore` suppression map** from the CST.
4. **Fill in the missing checks** — but be honest: the genuinely-missing ones split into
   *self-contained* (do now), *needs-W2-CFG* (`UNREACHABLE_*`, the `UNSAFE_*` suppression), and
   *needs-scene-typing* (`GET_NODE_DEFAULT_WITHOUT_ONREADY`).

The headline correction to the plan: **the "Phase-2 seam" is a fiction in the current tree** — there
is no `RawWarning`, no filter layer, no `WarningCode`. This playbook builds it. Until it exists,
"emit all 48" is meaningless because there is nowhere to gate them.

---

## 1. Current state — what EXISTS today vs the gap

### 1.1 The Diagnostic POD (`crates/gdscript-base/src/lib.rs:71-89`)

```rust
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: Severity,        // baked at emit time today
    pub code: String,              // <-- a bare string, NOT a typed enum
    pub message: String,
    #[serde(default)] pub source: DiagnosticSource,   // Syntax | Type
    #[serde(default)] pub fixes: Vec<CodeAction>,
}
pub enum Severity { Error, Warning, Info, Hint }   // line 48
```

`code: String` is the contract npm + Rust consumers key on. **This is a constraint, not a blank
slate:** the V1 plan sketches `pub code: WarningCode` on the POD, but changing `Diagnostic.code`'s
*type* is a **breaking POD change** (Workstream 6's contract). The pragmatic 1.0 shape: keep
`code: String` on the wire (the stable serialized identity, e.g. `"INTEGER_DIVISION"`), introduce
`WarningCode` as an **internal** enum in `gdscript-hir`, and let `WarningCode::as_str()` produce the
string. The enum is the single source of truth; the POD stays string-typed and stable. (See §3.1 —
this is a deliberate divergence from the plan's sketch, justified by the existing contract.)

### 1.2 How warnings are emitted today (the emit sites)

All emit sites live in `crates/gdscript-hir/src/infer.rs`. There are **two** emit paths:

**(a) The `Context::emit` helper** (`infer.rs:524-533`) — the body-walk diagnostics:

```rust
fn emit(&mut self, range: TextRange, severity: Severity, code: &str, message: String) {
    self.diagnostics.push(Diagnostic {
        range, severity, code: code.to_owned(), message,
        source: DiagnosticSource::Type, fixes: Vec::new(),
    });
}
```

Call sites of `emit` / direct pushes, with the **baked-in severity**:

| Site (infer.rs) | Code const | Severity baked | Notes |
|---|---|---|---|
| `check_assign` :543 | `NARROWING_CONVERSION` | Warning | float→int |
| `check_assign` :552 | `TYPE_MISMATCH` | **Error** | **NOT a Godot warning code** — our umbrella for engine `push_error` |
| `infer_local_var` :690 | `INFERENCE_ON_VARIANT` | **Error** | matches Godot default ERROR |
| `:935` | `INTEGER_DIVISION` | Warning | `int / int` |
| `:1085` | `INVALID_NODE_PATH` | Warning | **NOT a Godot warning** — our own; engine has no such code |
| `:1205` | `UNSAFE_CALL_ARGUMENT` | Warning | (Godot default is **IGNORE**) |
| `emit_unsafe` :1476 | `UNSAFE_PROPERTY_ACCESS` / `UNSAFE_METHOD_ACCESS` | Warning | (Godot default is **IGNORE**) |
| `analyze_file` :250 | `SHADOWED_GLOBAL_IDENTIFIER` | Warning | Godot emits as **error**; we soften to warning |
| `analyze_file` :273 | `CYCLIC_INHERITANCE` | Warning | **NOT a gateable Godot warning** — engine hard error |

**The code constants** are bare `pub const X: &str = "X";` at `infer.rs:28-56` (10 of them). So the
"~10 codes" the brief cites is accurate, but **three are not Godot gateable warnings**: `TYPE_MISMATCH`
(umbrella for hard type errors → `push_error`), `INVALID_NODE_PATH` (our own value-add; closest engine
code is `GET_NODE_DEFAULT_WITHOUT_ONREADY`, which is a *different* check), and `CYCLIC_INHERITANCE`
(engine emits it as an error, not a `debug/gdscript/warnings` entry). Only **7** map to the 48-set:
`INFERENCE_ON_VARIANT`, `NARROWING_CONVERSION`, `INTEGER_DIVISION`, `UNSAFE_PROPERTY_ACCESS`,
`UNSAFE_METHOD_ACCESS`, `UNSAFE_CALL_ARGUMENT`, `SHADOWED_GLOBAL_IDENTIFIER`.

**A plumbed-but-dead code:** `Assign::IntAsEnum` (`ty.rs:174,218`) is *computed* by `is_assignable`,
but the consuming arm at `infer.rs:561` (`Assign::Ok | Assign::OkUnsafe | Assign::IntAsEnum => {}`)
is a **no-op** — `INT_AS_ENUM_WITHOUT_CAST` is detected and then dropped. One of the cheapest "missing"
codes to land.

### 1.3 The diagnostic flow up to the client

```
infer::analyze_file(db, api, root, file_id)  -> FileInference { diagnostics: Vec<Diagnostic> }
  └─ queries::analyze_file(db, file) [#[salsa::tracked]]  (queries.rs:37)  -> Arc<FileInference>
       └─ ide::semantic::type_diagnostics(db, file)  (semantic.rs:47)  -> Vec<Diagnostic> (clone)
            └─ Analysis::diagnostics(file)  (ide/lib.rs:188)  -> parse diags ∪ type diags
```

**Key consequence for gating:** `queries::analyze_file` is `#[salsa::tracked]` and **memoizes
`Diagnostic`s with severity already resolved**. If `gate()` ran *inside* `analyze_file`, then editing a
warning setting would force `analyze_file` (and all of inference) to recompute — exactly the
salsa-cacheability violation §6 forbids. So **gating must live downstream of the tracked
`analyze_file` query** — in `ide::semantic::type_diagnostics` or a thin tracked wrapper keyed on the
settings input. This is the single most important architectural fact in this workstream and the plan
does not call out *where* in the query graph the seam sits.

### 1.4 Project config parsing (`crates/gdscript-hir/src/project.rs`)

Already present and the right pattern to extend:
- `parse_autoloads(text) -> Vec<AutoloadEntry>` (line 31) — the line-oriented `[section]`/`key=value`
  minimal scan; `dequote()` helper (line 125).
- `parse_engine_version(text) -> Option<(u32,u32)>` (line 80) — reads `[application]
  config/features`; `parse_major_minor` (line 117).

Surfaced as salsa-tracked queries keyed on `ProjectConfig` alone (`queries.rs`):
- `autoload_registry(db, config)` :172, `engine_version(db, config)` :192,
  `project_engine_version(db)` :199.

`ProjectConfig` is a `#[salsa::input]` (`gdscript-db/src/lib.rs:86`) holding `project_godot_text:
Arc<str>` at **MEDIUM durability** (`set_project_config`, lib.rs:234). **The gap:** there is **no**
`parse_warning_settings` and no `[debug] gdscript/warnings/*` reader. The `[debug]` section is never
scanned. This is net-new but follows `parse_autoloads`/`parse_engine_version` exactly.

### 1.5 Narrowing today (relevant to which warnings W1 can finish)

`infer.rs` does **syntactic, lexical** narrowing only: a `narrowing: FxHashMap<String, Ty>` keyed on a
dotted path (`narrow_key`, :1767), populated by `apply_narrowing(cond)` (:1743) inside a
branch-scoped clone frame (`in_branch`, :1819). It is widen-only and has **no CFG, no else-branch
facts, no early-return flow, no `match`-arm typing across statements**. W2 replaces this. The
consequence for W1: **`UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` cannot be implemented in W1** (they need
the CFG), and the *correct suppression* of `UNSAFE_*` inside guards (the #93510 win) is a W2
deliverable, not W1. W1 *defines and gates* those codes; W2 *makes them fire correctly*.

### 1.6 Fixtures / tests

`fixtures/warnings/` does **not** exist yet (only `fixtures/{ide,parser}/.gitkeep`). `tests/` is a lone
`.gitkeep`. The golden-corpus + differential harness this workstream needs is **all net-new
scaffolding**.

---

## 2. The 1.0 cut vs the deferred tail

**In the 1.0 cut (W1 owns):**
- The `WarningCode` enum + `gate()` + `WarningSettings` + the `@warning_ignore` suppression map — the
  whole emit-then-gate machinery.
- Every **self-contained** check that needs only the item-tree/single-body walk already present
  (the unused/unassigned/shadowing/numeric/assert/standalone/confusable/deprecated-keyword families —
  see §4 table for the exact DONE/PARTIAL/MISSING split).
- Project-setting gating for **all** codes (master switch, per-code override, `treat_as_errors`, the
  scope rules) and version-gating master-only codes via `WarningCode::since()`.

**Deferred to W2 (control-flow) — W1 only declares + gates these:**
`UNREACHABLE_CODE`, `UNREACHABLE_PATTERN`, and the *correct suppression* of `UNSAFE_PROPERTY_ACCESS` /
`UNSAFE_METHOD_ACCESS` inside proven guards.

**Deferred to scene typing (Phase 4 already landed; wiring is W1-adjacent):**
`GET_NODE_DEFAULT_WITHOUT_ONREADY` needs the scene model to know a `get_node(...)` default-value init.
We already parse scenes (`gdscript-scene`) and type `$Path`; this check is the onready-vs-default
reasoning on top.

**Explicitly NOT W1 (and not even gateable warnings):** `ABSTRACT_CLASS_INSTANTIATED` and
`RENAMED_IN_GODOT_4_HINT` (research/04 §2.2 — model the first as a semantic error if at all, ignore
the second). Our `TYPE_MISMATCH`, `INVALID_NODE_PATH`, `CYCLIC_INHERITANCE` are **analyzer value-add**,
not part of the 48; they keep their current codes and are **not** gated by `debug/gdscript/warnings/*`
(they have no engine setting key). Document them as analyzer-native diagnostics.

---

## 3. Design — concrete types/modules to add

### 3.1 `WarningCode` — a new module `crates/gdscript-hir/src/warnings.rs`

The single source of truth. Matches the existing idiom (bare consts today → a typed enum + tables),
but **does NOT change the POD**: the wire `code` stays `String` via `as_str()`.

```rust
// crates/gdscript-hir/src/warnings.rs  (new)

/// The gateable Godot warning codes (research/04 §2.2). Internal to `gdscript-hir`; the public
/// `Diagnostic.code` is its `as_str()` form, so the wire contract stays a stable string.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WarningCode {
    // Unassigned/unused
    UnassignedVariable, UnassignedVariableOpAssign, UnusedVariable, UnusedLocalConstant,
    UnusedPrivateClassVariable, UnusedParameter, UnusedSignal,
    // Shadowing
    ShadowedVariable, ShadowedVariableBaseClass, ShadowedGlobalIdentifier,
    // Control-flow (UNREACHABLE_* need W2)
    UnreachableCode, UnreachablePattern, StandaloneExpression, StandaloneTernary, IncompatibleTernary,
    // Type-safety
    UnsafeVoidReturn, StaticCalledOnInstance,
    // Tool/static/await
    MissingTool, RedundantStaticUnload, RedundantAwait,
    // Assertions
    AssertAlwaysTrue, AssertAlwaysFalse,
    // Numeric/enum
    IntegerDivision, NarrowingConversion, IntAsEnumWithoutCast, IntAsEnumWithoutMatch,
    EnumVariableWithoutDefault,
    // File/keyword
    EmptyFile, DeprecatedKeyword,
    // Confusables
    ConfusableIdentifier, ConfusableLocalDeclaration, ConfusableLocalUsage,
    ConfusableCaptureReassignment, ConfusableTemporaryModification, // last = master-only
    // Deprecated misuse (compiled out under DISABLE_DEPRECATED upstream)
    PropertyUsedAsFunction, ConstantUsedAsFunction, FunctionUsedAsProperty,
    // Type-strictness (opt-in, default IGNORE)
    UntypedDeclaration, InferredDeclaration, UnsafePropertyAccess, UnsafeMethodAccess,
    UnsafeCast, UnsafeCallArgument, ReturnValueDiscarded, MissingAwait, // MissingAwait master-only
    // Hard-fail (default ERROR)
    InferenceOnVariant, NativeMethodOverride, GetNodeDefaultWithoutOnready, OnreadyWithExport,
}

/// Mirrors Godot's `WarnLevel` (`gdscript_warning.h`): Ignore(0)/Warn(1)/Error(2).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WarnLevel { Ignore, Warn, Error }

/// Lowest Godot minor a code exists in (research/04 §2.2 master-only notes).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Since { V4_3, Master }   // Master = "newer than any 4.x stable we bundle as default"

impl WarningCode {
    /// The stable serialized identity, e.g. "INTEGER_DIVISION" — what `Diagnostic.code` carries.
    pub fn as_str(self) -> &'static str { /* match arm per variant */ }
    /// The `project.godot` key tail, e.g. "integer_division" (lowercased name).
    pub fn setting_name(self) -> &'static str { /* = as_str().to_ascii_lowercase(), precomputed */ }
    /// Godot's `default_warning_levels[]` entry.
    pub fn default_level(self) -> WarnLevel { /* §4 table */ }
    /// Version gate for master-only codes.
    pub fn since(self) -> Since { /* Master for CONFUSABLE_TEMPORARY_MODIFICATION, MISSING_AWAIT */ }
    /// Look up a code by its lowercased setting name (for parsing project.godot + @warning_ignore).
    pub fn from_setting_name(name: &str) -> Option<WarningCode> { /* reverse table */ }
}
```

> `as_str()`/`setting_name()` are exhaustive `match`es so adding a variant is a compile error until
> every table is updated — the same discipline the existing `Ty::label` uses. Generate the
> `from_setting_name` reverse map once (a `OnceLock<FxHashMap<&str, WarningCode>>` or a `match`).

The existing `pub const` codes in `infer.rs:28-56` are **replaced** by `WarningCode::as_str()` returns
(keep the three non-gateable ones — `TYPE_MISMATCH`, `INVALID_NODE_PATH`, `CYCLIC_INHERITANCE` — as
plain string consts; they are not in the enum).

### 3.2 `RawWarning` — decoupling emit from severity

The body walk currently calls `emit(range, Severity::Warning, CODE, msg)`. Change the *internal* emit
to record a code-keyed raw warning with **no severity**:

```rust
// In infer.rs — the new internal emit. Severity is resolved later by gate().
pub(crate) struct RawWarning {
    pub range: TextRange,
    pub code: WarningCode,
    pub message: String,
}
fn warn(&mut self, range: TextRange, code: WarningCode, message: String) {
    self.raw_warnings.push(RawWarning { range, code, message });
}
```

`InferenceResult`/`FileInference` carry `Vec<RawWarning>` instead of (or alongside, during migration)
`Vec<Diagnostic>`. The three **non-gateable analyzer diagnostics** (`TYPE_MISMATCH`,
`INVALID_NODE_PATH`, `CYCLIC_INHERITANCE`) keep emitting fully-formed `Diagnostic`s directly — they are
not gated, so they need no `RawWarning` round-trip. (Pragmatic migration: `FileInference` grows a
`raw_warnings: Vec<RawWarning>` field and keeps `diagnostics: Vec<Diagnostic>` for the ungated three;
the gate merges them.)

### 3.3 `WarningSettings` + the pure `gate()` — `warnings.rs`

```rust
pub struct WarningSettings {
    pub enabled: bool,                                   // debug/gdscript/warnings/enable (default true)
    pub treat_as_errors: bool,                           // .../treat_warnings_as_errors
    pub per_code: FxHashMap<WarningCode, WarnLevel>,     // explicit project.godot overrides
    pub scope: WarnScope,                                // ExcludeAddons | DirectoryRules
    pub engine: (u32, u32),                              // for since()-gating master-only codes
    /// Analyzer-default deviation (research/04 §2.3): when true (CLI --strict / standalone default
    /// with no project.godot), the opt-in IGNORE group is forced to Warn. A present project.godot
    /// clears this (its settings win).
    pub strict_opt_in: bool,
}
pub enum WarnScope { ExcludeAddons(bool), DirectoryRules(Vec<DirRule>) }

/// The ONLY place settings + version touch a warning. Pure → trivially cacheable & testable.
pub fn gate(raw: &RawWarning, s: &WarningSettings, ignores: &SuppressionMap, path: Option<&str>)
    -> Option<Diagnostic>
{
    if !s.enabled { return None; }
    // version gate: a master-only code never fires for a 4.3 project (and is absent on master? n/a).
    if raw.code.since() == Since::Master && s.engine < (4, 5) /* nearest "master-ish" cut */ { return None; }
    // base level: explicit override > strict-opt-in promotion > engine default.
    let mut level = s.per_code.get(&raw.code).copied()
        .unwrap_or_else(|| {
            let d = raw.code.default_level();
            if s.strict_opt_in && d == WarnLevel::Ignore && is_opt_in_group(raw.code) { WarnLevel::Warn } else { d }
        });
    if level == WarnLevel::Ignore { return None; }
    if s.treat_as_errors && level == WarnLevel::Warn { level = WarnLevel::Error; }
    if !s.scope.includes(path) { return None; }
    if ignores.is_suppressed(raw.code, raw.range) { return None; }
    Some(Diagnostic {
        range: raw.range,
        severity: match level { WarnLevel::Error => Severity::Error, _ => Severity::Warning },
        code: raw.code.as_str().to_owned(),
        message: raw.message.clone(),
        source: DiagnosticSource::Type,
        fixes: Vec::new(),
    })
}
```

`gate` is `WarnLevel::Ignore`/scope/suppression order matters: research/04 §2.3 precedence is
enable → per-code level → treat-as-errors → scope. The suppression map is consulted **last** (it
overrides everything, like Godot's `@warning_ignore`).

### 3.4 Parsing `debug/gdscript/warnings/*` — extend `project.rs`

A new `parse_warning_settings(text, engine) -> WarningSettings`, modeled on `parse_autoloads`:

```rust
pub fn parse_warning_settings(text: &str, engine: (u32, u32)) -> WarningSettings {
    // Track `[section]`; within `[debug]`, slash-keys flatten: `gdscript/warnings/enable=false`.
    // (project.godot stores `application/config/name` as the line `config/name=...` under
    // [application] — so under [debug] the keys read `gdscript/warnings/<tail>=<value>`.)
    //   gdscript/warnings/enable                  -> bool
    //   gdscript/warnings/treat_warnings_as_errors-> bool
    //   gdscript/warnings/exclude_addons          -> bool (4.3/4.4)
    //   gdscript/warnings/<code_setting_name>      -> 0|1|2 (Ignore/Warn/Error) via from_setting_name
    //   gdscript/warnings/directory_rules          -> dict (master; best-effort, see Risks)
}
```

Surface it as a salsa-tracked query keyed on `ProjectConfig` alone (mirrors `autoload_registry`):

```rust
// queries.rs
#[salsa::tracked]
pub fn warning_settings(db: &dyn Db, config: ProjectConfig) -> Arc<WarningSettings> {
    let engine = crate::project::parse_engine_version(config.project_godot_text(db))
        .unwrap_or(gdscript_api::GODOT_VERSION);
    Arc::new(crate::project::parse_warning_settings(config.project_godot_text(db), engine))
}
```

Because it is keyed on `ProjectConfig` (MEDIUM durability) and never reads a `.gd` body, **editing a
warning level invalidates only `warning_settings` and the downstream gate, never `analyze_file`/
`item_tree`/`infer`** — the salsa-cacheability requirement, satisfied by construction (§6).

### 3.5 The suppression map — `@warning_ignore[_start|_restore]`

`@warning_ignore` annotations are already lexed (they are `Annotation` CST nodes — the parser retains
all 36 annotations per research/04 §1.10). Build a per-file `SuppressionMap` from the CST:

```rust
pub struct SuppressionMap { spans: Vec<(TextRange, SmallVec<[WarningCode; 2]>)> }
impl SuppressionMap {
    fn is_suppressed(&self, code: WarningCode, at: TextRange) -> bool {
        self.spans.iter().any(|(span, codes)|
            span.start <= at.start && at.end <= span.end && codes.contains(&code))
    }
}
```

Construction rules (research/04 §1.10 #34–36, §2.3):
- `@warning_ignore("name", …)` → suppress the listed codes over the **single following
  statement/declaration's range** (the next sibling node after the annotation).
- `@warning_ignore_start("name", …)` … `@warning_ignore_restore("name", …)` → a **region** from the
  start annotation to the matching restore (per-code matching) or EOF.
- Names are the **lowercased setting names** → `WarningCode::from_setting_name`. An **unknown name** →
  a meta-diagnostic at the annotation argument (Godot warns on unknown ignore names — match it; emit
  as `UNUSED`/a dedicated code? Godot uses an `UNKNOWN_WARNING_IGNORE`-style message via `push_warning`
  — emit a plain analyzer warning with a clear message; it is not in the 48-set, so it is *not*
  itself gateable — document this).

Surface as a tracked query keyed on the **parse** (not the body), so it is part of the
firewall-friendly layer and recomputes only when the file text changes:

```rust
#[salsa::tracked]
pub fn suppression_map(db: &dyn Db, file: FileText) -> Arc<SuppressionMap> {
    Arc::new(crate::warnings::build_suppression_map(&parse(db, file).syntax_node()))
}
```

CST byte ranges are stable across incremental edits — the same property the existing
`source_map.expr_range` relies on.

### 3.6 Where the gate runs (the query-graph seam — the plan omits this)

Put the gate in `ide::semantic::type_diagnostics` (`crates/gdscript-ide/src/semantic.rs:47`), which
today is just `queries::analyze_file(db, file).diagnostics.clone()`:

```rust
// semantic.rs (rewritten)
pub fn type_diagnostics(db: &dyn Db, file: FileText) -> Vec<Diagnostic> {
    let inf = queries::analyze_file(db, file);          // memoized, severity-free raw warnings
    let settings = db.project_config()
        .map(|c| queries::warning_settings(db, c))
        .unwrap_or_else(|| Arc::new(WarningSettings::analyzer_default()));   // standalone default
    let ignores = queries::suppression_map(db, file);
    let path = db.file_text(file.file_id(db)).and_then(|ft| ft.res_path(db));
    let mut out: Vec<Diagnostic> = inf.diagnostics.clone();   // the ungated analyzer-native three
    out.extend(inf.raw_warnings.iter()
        .filter_map(|rw| crate::gate(rw, &settings, &ignores, path.as_deref())));
    out
}
```

Optionally wrap this in its own `#[salsa::tracked]` query so the *gated* result is itself memoized;
its inputs are `analyze_file` (LOW, body-driven) + `warning_settings` (MEDIUM) + `suppression_map`
(LOW, parse-driven). A warning-setting edit invalidates only `warning_settings` → this wrapper, **not**
`analyze_file`. Verify with a query-recount test (§5.2).

---

## 4. The complete warning table — DONE / PARTIAL / MISSING (grounded)

Default level from Godot `default_warning_levels[]` (research/04 §2.2). "Status" is against the
**current** tree.

| Code | Default | Status | What's needed in W1 |
|---|---|---|---|
| `UNASSIGNED_VARIABLE` | WARN | **MISSING** | item-tree/body walk: `var x:T` used before assign |
| `UNASSIGNED_VARIABLE_OP_ASSIGN` | WARN | **MISSING** | `x += …` on an unassigned local |
| `UNUSED_VARIABLE` | WARN | **MISSING** | local `var` never read (bindings already tracked in `InferenceResult.bindings`) |
| `UNUSED_LOCAL_CONSTANT` | WARN | **MISSING** | as above for `const` |
| `UNUSED_PRIVATE_CLASS_VARIABLE` | WARN | **MISSING** | `_name` member never read in-class |
| `UNUSED_PARAMETER` | WARN | **MISSING** | param never read (skip `_`-prefixed) |
| `UNUSED_SIGNAL` | WARN | **MISSING** | `signal` never `.emit`/connected in-file |
| `SHADOWED_VARIABLE` | WARN | **MISSING** | local shadows an outer local |
| `SHADOWED_VARIABLE_BASE_CLASS` | WARN | **MISSING** | member shadows a base member (needs item-tree of base — already resolvable via `script_class`) |
| `SHADOWED_GLOBAL_IDENTIFIER` | WARN | **PARTIAL** | exists for `class_name` collision (`infer.rs:250`); engine also fires for a *local/member* shadowing a global — extend |
| `UNREACHABLE_CODE` | WARN | **W2** | needs CFG |
| `UNREACHABLE_PATTERN` | WARN | **W2** | `match` arm after wildcard/bind — needs CFG/match analysis |
| `STANDALONE_EXPRESSION` | WARN | **MISSING** | `Stmt::Expr` whose value is unused & side-effect-free |
| `STANDALONE_TERNARY` | WARN | **MISSING** | a ternary as a statement (body already lowers ternary, `body.rs:212`) |
| `INCOMPATIBLE_TERNARY` | WARN | **MISSING** | mismatched arm types (infer both arms, compare) |
| `UNSAFE_VOID_RETURN` | WARN | **MISSING** | `return f()` where `f` returns Variant into a `-> void` |
| `STATIC_CALLED_ON_INSTANCE` | WARN | **MISSING** | calling a static method through an instance |
| `MISSING_TOOL` | WARN | **MISSING** | base `@tool` w/o local `@tool` (needs base item-tree + annotation) |
| `REDUNDANT_STATIC_UNLOAD` | WARN | **MISSING** | `@static_unload` w/ no static vars |
| `REDUNDANT_AWAIT` | WARN | **MISSING** | `await` on a non-coroutine/non-signal |
| `ASSERT_ALWAYS_TRUE` | WARN | **MISSING** | `assert(true)` / always-true constant (`Stmt::Assert` exists, `body.rs:413`) |
| `ASSERT_ALWAYS_FALSE` | WARN | **MISSING** | `assert(false)` |
| `INTEGER_DIVISION` | WARN | **DONE** | emitted `infer.rs:935` — re-route through `gate` |
| `NARROWING_CONVERSION` | WARN | **DONE** | `infer.rs:543` — re-route |
| `INT_AS_ENUM_WITHOUT_CAST` | WARN | **PARTIAL (dead)** | `Assign::IntAsEnum` computed (`ty.rs:218`) but the emit arm is a no-op (`infer.rs:561`) — wire it |
| `INT_AS_ENUM_WITHOUT_MATCH` | WARN | **MISSING** | int compared to enum in a `match` |
| `ENUM_VARIABLE_WITHOUT_DEFAULT` | WARN | **MISSING** | `var e: SomeEnum` w/o initializer |
| `EMPTY_FILE` | WARN | **MISSING** | trivial: file with no members |
| `DEPRECATED_KEYWORD` | WARN | **MISSING** | `yield` (parser must surface it) |
| `CONFUSABLE_IDENTIFIER` | WARN | **MISSING** | mixed-script/homoglyph identifiers (Unicode confusables) |
| `CONFUSABLE_LOCAL_DECLARATION` | WARN | **MISSING** | local declared after a same-name outer use |
| `CONFUSABLE_LOCAL_USAGE` | WARN | **MISSING** | use-before-decl of a local shadowing a member |
| `CONFUSABLE_CAPTURE_REASSIGNMENT` | WARN | **MISSING** | reassigning a lambda capture |
| `CONFUSABLE_TEMPORARY_MODIFICATION` | WARN | **MISSING** *(master-only)* | `since=Master` gate |
| `PROPERTY_USED_AS_FUNCTION` | WARN | **MISSING** *(deprecated)* | `obj.prop()` where prop is a property |
| `CONSTANT_USED_AS_FUNCTION` | WARN | **MISSING** *(deprecated)* | |
| `FUNCTION_USED_AS_PROPERTY` | WARN | **MISSING** *(deprecated)* | |
| `UNTYPED_DECLARATION` | **IGNORE** | **MISSING** | `var x = …` w/o `:T` (the bindings carry `annotated`, `infer.rs:81`) |
| `INFERRED_DECLARATION` | **IGNORE** | **MISSING** | `:=` declaration (`inferred_colon_eq` flag exists, `infer.rs:84`) |
| `UNSAFE_PROPERTY_ACCESS` | **IGNORE** | **DONE (over-eager)** | emitted `emit_unsafe` :1476 — re-route + W2 suppression |
| `UNSAFE_METHOD_ACCESS` | **IGNORE** | **DONE (over-eager)** | as above |
| `UNSAFE_CAST` | **IGNORE** | **MISSING** | `as T` through a Variant |
| `UNSAFE_CALL_ARGUMENT` | **IGNORE** | **DONE** | `infer.rs:1205` — re-route |
| `RETURN_VALUE_DISCARDED` | **IGNORE** | **MISSING** | a non-void call result dropped |
| `MISSING_AWAIT` | **IGNORE** | **MISSING** *(master-only)* | `since=Master` |
| `INFERENCE_ON_VARIANT` | **ERROR** | **DONE** | `infer.rs:690` — re-route (level=Error from default) |
| `NATIVE_METHOD_OVERRIDE` | **ERROR** | **MISSING** | overriding a native virtual with a wrong signature |
| `GET_NODE_DEFAULT_WITHOUT_ONREADY` | **ERROR** | **scene-dep** | needs scene model + `@onready` reasoning |
| `ONREADY_WITH_EXPORT` | **ERROR** | **MISSING** | `@onready` + `@export` on one member (annotation pair) |

**Tally:** DONE = 5 (`INTEGER_DIVISION`, `NARROWING_CONVERSION`, `UNSAFE_CALL_ARGUMENT`,
`INFERENCE_ON_VARIANT`, the two `UNSAFE_*` accesses); PARTIAL = 2 (`SHADOWED_GLOBAL_IDENTIFIER`
class-name-only, `INT_AS_ENUM_WITHOUT_CAST` dead arm); W2 = 2; scene-dep = 1; the rest **missing**.
The **machinery** (§3) is the gating prerequisite for *all* of them; the individual checks are
independent and can be sequenced by value (start with the trivial: `EMPTY_FILE`, `UNUSED_*`,
`UNTYPED_DECLARATION`/`INFERRED_DECLARATION` from existing binding flags, `INT_AS_ENUM_WITHOUT_CAST`
arm).

---

## 5. Step-by-step implementation plan

### M0 — the gating skeleton (no new checks; re-route the 5 DONE codes)
1. Add `crates/gdscript-hir/src/warnings.rs`: `WarningCode` (all 48 + tables: `as_str`,
   `setting_name`, `default_level`, `since`, `from_setting_name`), `WarnLevel`, `Since`,
   `RawWarning`, `WarningSettings` (+ `analyzer_default()` / `engine_default()`), `WarnScope`,
   `SuppressionMap`, and the pure `gate()`. Register the module in `hir/src/lib.rs`.
2. Convert the 5 DONE emit sites in `infer.rs` from `emit(range, Severity::_, CODE, msg)` to
   `warn(range, WarningCode::_, msg)`; add `raw_warnings: Vec<RawWarning>` to `InferenceResult` +
   `FileInference`; keep the three analyzer-native ones (`TYPE_MISMATCH`, `INVALID_NODE_PATH`,
   `CYCLIC_INHERITANCE`) emitting `Diagnostic` directly.
3. Add `parse_warning_settings` to `project.rs` + the `warning_settings` tracked query (`queries.rs`).
4. Add `build_suppression_map` (`warnings.rs`) + the `suppression_map` tracked query.
5. Rewrite `ide::semantic::type_diagnostics` to run `gate()` (§3.6). **Exit:** existing tests pass
   with identical output when no `project.godot` settings are present (analyzer default), and a
   synthetic `project.godot` with `gdscript/warnings/enable=false` produces zero type diagnostics.

### M1 — the self-contained checks (no CFG, no scene)
Land in value order, each with a fixture pair (§6): `EMPTY_FILE`; the `UNUSED_*` family (read-use
analysis over `InferenceResult.bindings` + member refs); `UNTYPED_DECLARATION` / `INFERRED_DECLARATION`
(directly from the `annotated` / `inferred_colon_eq` binding flags); `INT_AS_ENUM_WITHOUT_CAST` (wire
the dead arm at `infer.rs:561`); `SHADOWED_VARIABLE` / `SHADOWED_VARIABLE_BASE_CLASS`; the
`ASSERT_ALWAYS_*` pair (`Stmt::Assert`); `STANDALONE_EXPRESSION` / `STANDALONE_TERNARY`;
`INCOMPATIBLE_TERNARY`; `DEPRECATED_KEYWORD` (`yield`); `ONREADY_WITH_EXPORT`,
`REDUNDANT_STATIC_UNLOAD`, `MISSING_TOOL` (annotation-pair checks); `ENUM_VARIABLE_WITHOUT_DEFAULT`;
`RETURN_VALUE_DISCARDED`, `UNSAFE_VOID_RETURN`, `UNSAFE_CAST`; the `CONFUSABLE_*` family; the
deprecated-misuse trio; `NATIVE_METHOD_OVERRIDE`; `STATIC_CALLED_ON_INSTANCE`; `REDUNDANT_AWAIT`.
Extend `SHADOWED_GLOBAL_IDENTIFIER` beyond class-name collisions to local/member-shadows-global.

### M2 — gating completeness + suppression
Per-code overrides, `treat_warnings_as_errors`, `exclude_addons` (4.3/4.4) / `directory_rules`
(master) by file path; the full `@warning_ignore[_start|_restore]` map incl. unknown-name
meta-diagnostic; the `--strict` / `--engine-defaults` analyzer-default deviation wiring (the CLI flag
already reserved in `PHASE-5-CLI-PLAYBOOK.md` §1). Version-gate `CONFUSABLE_TEMPORARY_MODIFICATION` /
`MISSING_AWAIT` via `since()`.

### M3 — the W2/scene-dependent codes (coordinated)
`UNREACHABLE_CODE` / `UNREACHABLE_PATTERN` land *with* W2's CFG (W1 has already declared + gated them).
The `UNSAFE_*` suppression-inside-guards (#93510) is W2's narrowing feeding the existing
`emit_unsafe`. `GET_NODE_DEFAULT_WITHOUT_ONREADY` consumes the scene model (Phase 4).

Per-milestone: an adversarial bug-hunt pass (find → verify → fix), matching every prior milestone.

---

## 6. Test plan

1. **Per-code golden corpus.** New `fixtures/warnings/<code>.gd` + `<code>.expected` (the dir does not
   exist yet — create it). Each `.expected` asserts **code + byte range + verbatim message + resolved
   severity** under the engine-default settings. One fixture per code (47 on 4.3, +1 master).
2. **Salsa-cacheability (the load-bearing test).** A query-recount test (mirroring
   `queries.rs:1515 observe_autoload_registry` and the `engine_version_…_firewalled_against_body_edits`
   test at `queries.rs:1410`): set a `project.godot`, run `type_diagnostics`, then **edit a warning
   level via `set_project_config`** and assert `analyze_file` / `item_tree` / `infer` did **not**
   recompute (only `warning_settings` + the gate wrapper did). This is the §3.4/§3.6 invariant and the
   plan's explicit constraint — it must have a dedicated counter test.
3. **Gating + suppression matrix.** Fixtures pairing a `.gd` with a synthetic `project.godot`:
   `enable=false` ⇒ zero; per-code override flips level (Ignore/Warn/Error); `treat_warnings_as_errors`
   escalates every WARN; `exclude_addons` / `directory_rules` suppress by `res://addons/...` path;
   `@warning_ignore` one-shot suppresses exactly the next stmt; `_start`/`_restore` suppress the region
   (and EOF-terminate); unknown ignore name → meta-diagnostic; a master-only code is silent on a 4.3
   `config/features`.
4. **Differential harness vs real Godot** (the Testing-strategy #1 from the V1 plan). Run the typed
   corpus through `godot --check-only` (or editor export) per supported minor; diff against our output
   with **two documented expected divergences**: (a) the `UNSAFE_*` cases W2 suppresses via narrowing
   (#93510), (b) default-level normalization (we normalize the opt-in group before comparing). Gate
   master-only codes to master only. **Honest note:** this harness needs a Godot binary in CI — it is a
   *separate, opt-in* CI job (not the default `xtask ci`), because the analyzer's whole value is *not*
   needing the engine. Document it as a parity-verification job, not a build dependency.
5. **`gate()` property tests.** `gate` is pure: property-test that (a) `enable=false` ⇒ always `None`;
   (b) an `Ignore`-level code is never emitted regardless of `treat_as_errors`; (c) `treat_as_errors`
   only ever escalates Warn→Error, never touches Error/Ignore; (d) a suppressed range always drops.
6. **POD-stability guard.** A test asserting `WarningCode::as_str()` is stable for every variant (a
   golden list of strings) — these are the consumer-facing identifiers Workstream 6 freezes; a typo or
   rename is a contract break, caught here, not in the field.

---

## 7. Risks + mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **Gating inside the tracked `analyze_file`** would make a warning-setting edit re-run inference (the salsa violation). | **Critical** | Gate **downstream** of `analyze_file`, in `type_diagnostics`, keyed on the MEDIUM-durability `warning_settings`; prove with the query-recount test (§6.2). The plan asserts the constraint but never says *where* the gate runs — this playbook fixes the seam at `semantic.rs`. |
| **POD `code` type change.** The plan sketches `pub code: WarningCode` on `Diagnostic`; that is a breaking change to the frozen `gdscript-base` POD (Workstream 6). | High | Keep `code: String` on the wire; `WarningCode` is internal; `as_str()` bridges. The §6.6 string-stability test guards the identifiers. |
| **The "Phase-2 emit-then-gate seam" does not exist in the tree.** Severity is baked at emit. | High | M0 builds it before any new check; `RawWarning` decouples emit from severity; migrate the 5 DONE codes first to validate the seam end-to-end before fanning out. |
| **Message drift vs the engine.** Godot changes strings between minors; the int enum shifts (4.3↔master). | Med | Key on the symbolic `WarningCode`/`setting_name`, never the int (research/04 §2.2); message is explicitly *not* the semver contract (Workstream 6); differential harness per minor catches drift; `since()` version-gates master-only codes. |
| **`directory_rules` (master) is a typed-Variant dict** — harder than the bool `exclude_addons`. | Med | Best-effort parse (we already do a minimal non-`VariantParser` scan in `project.rs`); fall back to "exclude `res://addons` by default" if the dict doesn't parse; cover the common case, document the limitation. |
| **`@warning_ignore` "next statement" span resolution** (which CST node the annotation guards). | Med | Use the next-sibling declaration/statement node's range from the CST; fixtures for one-shot vs region vs EOF-terminated; an unknown-name meta-diagnostic matches Godot. |
| **Over-eager `UNSAFE_*` today** fires inside guards the engine-default would silence anyway (they're IGNORE by default), and we currently bake them as Warning. | Med | Re-routing through `gate` immediately fixes the *level* (IGNORE by default ⇒ silent unless `--strict`); W2 fixes the *false positives*. The two effects are independent — gating helps before W2 lands. |
| **`TYPE_MISMATCH`/`INVALID_NODE_PATH`/`CYCLIC_INHERITANCE` are not Godot warnings** and have no setting key. | Low | Keep them as analyzer-native diagnostics, ungated, documented as value-add (Workstream 5 warning reference flags them as "analyzer-only"); do not invent fake `debug/gdscript/warnings` keys for them. |

---

## 8. Dependencies on other workstreams

- **W2 (control-flow narrowing)** — *blocks*: `UNREACHABLE_CODE`, `UNREACHABLE_PATTERN`, and the
  correct (#93510) suppression of `UNSAFE_PROPERTY_ACCESS`/`UNSAFE_METHOD_ACCESS`. W1 declares + gates
  these; W2 makes them fire correctly. The `emit_unsafe` site (`infer.rs:1459`) is the shared seam.
- **Phase 4 scene typing (landed)** — `GET_NODE_DEFAULT_WITHOUT_ONREADY` needs the scene model
  (`gdscript-scene`) + `@onready` reasoning. Wiring is W1-adjacent but depends on scene-typed node
  paths already in place.
- **Workstream 5 (docs)** — the per-warning reference is **generated from the `WarningCode` table**
  (single source of truth), so §3.1's `as_str`/`default_level`/`setting_name`/`since` must be complete
  and stable; the two documented divergences (#93510 suppression, default-level normalization) are
  authored here for the reference pages.
- **Workstream 6 (API/contract)** — `WarningCode::as_str()` strings are the **stable identifiers** the
  1.0 semver policy freezes (adding a code = MINOR; renaming = MAJOR). Keep `Diagnostic.code: String`;
  do not change the POD type. The §6.6 string-stability test feeds `cargo-semver-checks`'s human side.
- **Phase 5 CLI** (`PHASE-5-CLI-PLAYBOOK.md`) — already reserves `--engine-defaults` vs `--strict`;
  W1's `WarningSettings::{engine_default, analyzer_default}` + `strict_opt_in` realize them.
- **Godot-sync** ([`GODOT-SYNC.md`](GODOT-SYNC.md)) — surfaces per-release message + default-level
  deltas so the `WarningCode` tables and the differential harness track the engine.
