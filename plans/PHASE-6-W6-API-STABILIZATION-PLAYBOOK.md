# Phase 6 · Workstream 6 — API stabilization & the 1.0 commitment (Playbook)

> The **irreversible** workstream. At 1.0 the analyzer's public surface becomes a contract that a
> **2.0** is needed to break. This playbook inventories the *actual* surface in the code today,
> corrects where [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 6 has drifted from it,
> and gives an engineer the exact `#[non_exhaustive]` edits, the API-review checklist, the verbatim
> policy text, the supported-Godot matrix, and the `cargo-semver-checks` gate wiring to execute.
>
> **Parents / required reading:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 6 +
> §"What 1.0 commits us to"; [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2 (the API rules) + §9
> (SemVer row); [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md)
> §3.3 (0.x→1.0 reading). Format bar: the Phase-5 playbooks
> ([`PHASE-5-LSP-PLAYBOOK.md`](PHASE-5-LSP-PLAYBOOK.md), [`-CLI-`](PHASE-5-CLI-PLAYBOOK.md),
> [`-NAPI-`](PHASE-5-NAPI-PLAYBOOK.md)).

---

## 0. Thesis

The functional surface is **already built and stable in shape** — `AnalysisHost`/`Analysis` and a
full set of serde POD result structs ship today at `0.2.1`. Workstream 6 is therefore **not** a
design task; it is a **freeze-and-fortify** task with exactly three load-bearing pieces of real work:

1. **Add `#[non_exhaustive]` to every consumer-matched POD type** — the codebase has it on **zero**
   types today (verified: the only `non_exhaustive` token in the tree is an unrelated
   `finish_non_exhaustive()` Debug impl in `gdscript-db`). Without it, every future enum variant or
   struct field is a **major** bump. This is the single highest-leverage, lowest-risk edit in the
   whole phase, and it **must** land before the 1.0 cut because adding it *after* 1.0 is itself a
   breaking change.
2. **Correct the contract's *scope*.** The plan says "the `gdscript-ide` crate's public surface" is
   the contract. In the actual code, `gdscript-ide` **re-exports nothing** — every result type a
   consumer touches lives in the **separately-published** `gdscript-base` crate, which consumers
   `use gdscript_base::{Diagnostic, …}` directly. The contract is therefore **`gdscript-ide` +
   `gdscript-base`**, and the policy text must say so or it is wrong on day one.
3. **Run `cargo-semver-checks` where it actually catches breaks — on PRs, not only at release.** It
   is enabled in `release-plz.toml` (`semver_check = true`) but that only fires when a Release PR is
   *cut*; there is **no PR-time gate**, so a breaking change merges freely and is only caught when the
   release is assembled. Add a `semver` CI job.

Everything else (the policy statement, the Godot matrix, the API-review pass) is documentation and a
checklist over a surface that is already the right shape.

### The 1.0 cut vs the deferred tail

| In the 1.0 cut (this workstream) | Deferred (post-1.0) |
|---|---|
| `#[non_exhaustive]` on all consumer-matched POD + the `Change` builder | A typed `WarningCode` enum replacing `Diagnostic.code: String` (W1 owns this; see §3.4) — land it *with* W1 so the `#[non_exhaustive]` is set once |
| The contract scoped to **`gdscript-ide` + `gdscript-base`**, documented | Splitting POD into a `gdscript-ide`-re-exported façade so `gdscript-base` could become private (a 2.0-era refactor) |
| The verbatim semver/stability policy on the contract page | `cargo-public-api` snapshot tests (nice-to-have; `cargo-semver-checks` is the gate) |
| The supported-Godot matrix + a **public** `engine_version`/`bundled_godot_version` accessor on `Analysis` (does **not** exist yet — §3.5) | Per-project version *override* surfaced through the public API |
| `cargo-semver-checks` as a **PR gate** + the release gate | OIDC Trusted Publishing migration (already noted in release-plz.yml) |
| The API-review pass + the `#[non_exhaustive]` audit test | Deeper FFI schema codegen (TS `.d.ts` from the Rust POD) |

---

## 1. Current state — the actual public surface (verified, file:line)

### 1.1 `gdscript-ide` (`crates/gdscript-ide/src/lib.rs`) — the API entry points

Reachable `pub` items from the crate root:

| Item | Kind | Location | Notes |
|---|---|---|---|
| `AnalysisHost` | struct | `lib.rs:46` | `#[derive(Debug, Clone, Default)]`; one field `db` is private. |
| `AnalysisHost::new` | fn | `lib.rs:98` | |
| `AnalysisHost::apply_change` | fn | `lib.rs:105` | the only mutation entry point. |
| `AnalysisHost::set_engine_api` | fn | `lib.rs:142` | `(&[u8]) -> bool` — the wasm engine-blob install. |
| `AnalysisHost::analysis` | fn | `lib.rs:154` | `-> Analysis`. |
| `Change` | struct | `lib.rs:52` | **3 `pub` fields**: `files`, `paths`, `project_config` — see §1.4 risk. |
| `Change::{new,change_file,remove_file,set_file_path,set_project_config}` | fns | `lib.rs:65-93` | builder. |
| `Analysis` | struct | `lib.rs:165` | `#[derive(Debug, Clone)]`; private `db`. |
| `Analysis::syntax_tree` | fn | `lib.rs:176` | `Cancellable<Option<String>>`. |
| `Analysis::diagnostics` | fn | `lib.rs:188` | `Cancellable<Vec<Diagnostic>>`. |
| `Analysis::document_symbols` | fn | `lib.rs:205` | |
| `Analysis::semantic_tokens` | fn | `lib.rs:220` | `Cancellable<Vec<SemanticToken>>`. |
| `Analysis::folding_ranges` | fn | `lib.rs:233` | |
| `Analysis::completions` | fn | `lib.rs:248` | |
| `Analysis::hover` | fn | `lib.rs:266` | `Cancellable<Option<HoverResult>>`. |
| `Analysis::inlay_hints` | fn | `lib.rs:279` | |
| `Analysis::signature_help` | fn | `lib.rs:292` | |
| `Analysis::code_actions` | fn | `lib.rs:304` | takes `FilePosition` (the plan §2 sketch wrote `FileRange` — code wins). |
| `Analysis::goto_definition` | fn | `lib.rs:317` | `Cancellable<Vec<NavTarget>>`. |
| `Analysis::find_references` | fn | `lib.rs:325` | |
| `Analysis::rename` | fn | `lib.rs:335` | `Cancellable<Result<SourceChange, RenameError>>`. |
| `Analysis::workspace_symbols` | fn | `lib.rs:347` | |

`catch` (`lib.rs:37`) is **private** — good. The four feature modules (`features`, `navigation`,
`semantic`, `semantic_tokens`) are `mod` (private) — good. **`gdscript-ide` re-exports no types of
its own**; the result types are named via `gdscript_base::…` paths in the signatures, so a consumer
*must* depend on `gdscript-base` to name a `Diagnostic`. (This is the scope correction in §0.2.)

**Cancellable coverage:** every one of the 15 read queries returns `Cancellable<T>` — the plan's
checklist item "Cancellable on every read" is **already satisfied**; the review just confirms it.

### 1.2 `gdscript-base` (`crates/gdscript-base/src/lib.rs`) — the POD contract

Every `pub` result type, all `#[derive(Serialize, Deserialize)]`, all byte-offset based. **None has
`#[non_exhaustive]`.** Grouped by growth risk (the §3.1 plan):

| Type | Loc | Kind | Will grow? → needs `#[non_exhaustive]`? |
|---|---|---|---|
| `FileId(pub u32)` | `:17` | tuple struct | No — opaque handle, shape fixed. **Leave as-is** (adding `#[non_exhaustive]` to a 1-field tuple struct only blocks the struct-literal `FileId(0)` everyone uses; it is *meant* to be constructed). |
| `TextRange { start, end }` | `:21` | struct | No — a byte interval is closed by definition. Leave constructible. |
| `FilePosition { file, offset }` | `:38` | struct | No — closed. Leave constructible. |
| `Severity` | `:48` | enum | **YES.** Variants `Error, Warning, Info, Hint`. (⚠ plan §6.1 sketch writes `Information` — the code says **`Info`**; the policy/docs must use `Info`.) |
| `DiagnosticSource` | `:63` | enum | **YES** — `Syntax, Type`; a third source (e.g. `Scene`/`Format`) is plausible. |
| `Diagnostic` | `:73` | struct | **YES** — fields will grow (`tags`, `related`, a typed code). |
| `SymbolKind` | `:94` | enum | **YES** — 8 variants; more GDScript kinds plausible. |
| `DocumentSymbol` | `:115` | struct | **YES.** |
| `FoldKind` | `:133` | enum | **YES.** |
| `FoldRange` | `:144` | struct | borderline; mark it (cheap, future-proof). |
| `CompletionKind` | `:154` | enum | **YES.** |
| `CompletionItem` | `:175` | struct | **YES** (will gain `documentation`, `sort_text`, `deprecated`). |
| `HoverResult` | `:201` | struct | **YES.** |
| `ParamInfo` / `SignatureInfo` / `SignatureHelp` | `:214/:223/:235` | structs | **YES.** |
| `InlayHintKind` | `:248` | enum | **YES.** |
| `InlayHint` | `:257` | struct | **YES** (will gain `tooltip`, `padding`). |
| `SemanticTokenType` | `:271` | enum | **YES** — 15 variants, the most likely to grow. |
| `SemanticToken` | `:320` | struct | borderline (3 fields, all needed to decode); mark it. |
| `TextEdit` / `FileEdit` / `SourceChange` | `:331/:340/:350` | structs | **YES** for `SourceChange` (may gain rename-file ops); `TextEdit`/`FileEdit` are closed edit atoms — see §1.4 note. |
| `FileRange` | `:367` | struct | closed (file+range). |
| `ReferenceKind` | `:377` | enum | **YES.** |
| `Reference` | `:388` | struct | **YES.** |
| `RenameError` | `:400` | enum | **YES** — 4 variants today; new refusal reasons are certain. (Already `#[serde(tag="kind")]` — adjacently tagged, so a new variant is forward-compatible JSON.) |
| `CodeAction` | `:429` | struct | **YES.** |
| `NavTarget` | `:441` | struct | **YES** (may gain `container_name`). |
| `Cancelled` | `:457` | unit struct | No. |
| `Cancellable<T>` | `:460` | type alias | No. |
| `semantic_token_modifier` | `:305` | `mod` of `pub const u32` | The bitflags — adding a const is **already** non-breaking; document the bit layout as append-only. |
| `LineIndex` / `LineCol` | `:468/:477` | struct | `LineCol` is constructible POD (closed); `LineIndex` has private fields + methods (already encapsulated). |

The honest split: **enums and result-structs that a consumer pattern-matches or relies on field
completeness get `#[non_exhaustive]`; the small constructible "value" types** (`FileId`, `TextRange`,
`FilePosition`, `FileRange`, `LineCol`, and the edit atoms `TextEdit`/`FileEdit`) **stay
constructible** — a consumer building a `FilePosition { file, offset }` to call a query is the
intended use, and `#[non_exhaustive]` would break that ergonomics for no growth benefit.

### 1.3 The FFI POD JSON shape (`gdscript-session` + `gdscript-ffi` + `bindings/wasm`)

The npm contract is **JSON strings of the same `gdscript-base` POD**, produced in
`crates/gdscript-session/src/lib.rs`:

- Array queries → `serde_json::to_string(&Vec<Pod>)`, `"[]"` fallback (`lib.rs:121-284`).
- Option queries (`hover`, `signatureHelp`, `syntaxTree`) → `Option<String>` → JS `null`/`undefined`.
- `rename` → a hand-built **envelope** `{"ok": <SourceChange>}` | `{"error": <RenameError|string>}`
  (`lib.rs:262-271`) — note this is a **session-level shape not present in the Rust `Analysis::rename`
  signature** (which returns `Result<SourceChange, RenameError>`); the envelope is its own contract
  surface and must be in the documented schema.
- `gdscript-ffi` (`AnalysisHandle`, napi) and `bindings/wasm` (`Analyzer`, wasm-bindgen) are
  **byte-for-byte delegators** to `Session` — they add no shapes, only camelCase method names
  (`openDocument`, `documentSymbols`, …). So the JSON schema is defined **once** by `gdscript-base`'s
  serde + the rename envelope.

**Consequence for the contract:** the FFI JSON shape is a *projection of the same serde derives*. The
`#[non_exhaustive]` edits in `gdscript-base` keep the **Rust** side additive; serde's default
behavior (deserializing ignores unknown fields, serializing adds fields) keeps the **JSON** side
additive **as long as** we only *add* optional fields/variants — which the policy must state.

### 1.4 Known surface hazards found in the inventory

- **`Change` has 3 `pub` fields** (`files`, `paths`, `project_config`, `lib.rs:54-62`). These are
  public data a consumer can construct a `Change` literal from, bypassing the builders. At 1.0 that
  locks the field layout. **Fix:** add `#[non_exhaustive]` to `Change` so the builders
  (`change_file`/`set_file_path`/…) are the only stable construction path and fields can be added.
  (The fields stay `pub` for read access; `#[non_exhaustive]` blocks only the struct-literal
  construction outside the crate.)
- **`Severity` variant naming:** code = `Info`, plan = `Information`. LSP's wire name is
  `Information`; clients already map `Info → DiagnosticSeverity::INFORMATION` at their edge. **Do not
  rename** at 1.0 (it would be a break); document the canonical name as `Info`.
- **`Diagnostic.code: String`** today (`gdscript-base:79`), not a typed enum. W1 plans a
  `WarningCode` enum. The two workstreams must coordinate (§3.4) so the type of `code` is frozen
  **once**, with `#[non_exhaustive]`, at the cut — not changed in a later minor (changing a public
  field's type is a major bump).

### 1.5 Tooling state (verified)

- `release-plz.toml` → `semver_check = true` (`:26`) — runs `cargo-semver-checks` **at Release-PR
  time only**. Single shared workspace version `0.2.1`. Single `v{version}` tag on `gdscript-ide`.
- **No `semver`/`public-api` job in `.github/workflows/ci.yml`** (grep: zero hits) — the PR-time gap.
- **Published crates** (no `publish = false`): `gdscript-base, -syntax, -api, -db, -hir, -ide, -scene`
  → **all 7 are on crates.io and all 7 are semver-checked.** `publish = false`: `gdscript-session,
  -ffi, -wasm, -cli, -lsp` (`grep publish` confirms). This **contradicts** the plan's "internal
  crates are free to change" — see §6 Risk R1.
- `engine_version` query exists at `crates/gdscript-hir/src/queries.rs:192` and
  `project_engine_version` at `:199`, but **both are in the internal `gdscript-hir` crate** and are
  **not surfaced on `Analysis`**. `godot_version()` (the *bundled* API version) is at
  `crates/gdscript-api/src/lib.rs:38`, also internal. **The supported-version matrix has no public
  API hook today** — §3.5 adds one.
- `lsp-types` does **not** leak into `gdscript-base`/`gdscript-ide` (grep clean; the only hit is a
  doc-comment saying "never lsp-types"). Plan checklist item satisfied.

---

## 2. Goal (restated precisely against the code)

Produce, before the 1.0 tag, an **immutable, documented, enforced** public contract:

1. **Frozen surface:** `gdscript-ide` (entry points) **+ `gdscript-base`** (POD) **+ the FFI JSON
   shape** (the serde projection + the rename envelope). Every consumer-matched type carries
   `#[non_exhaustive]`; the constructible value types deliberately do not.
2. **A verbatim semver/stability policy** (§4) on a docs "contract page" and in `gdscript-ide`'s
   crate-level rustdoc, correctly scoped to **two** crates, naming what is *excluded* (internals,
   message text, inference precision).
3. **The supported-Godot matrix** (§5) tied to a **new public** `Analysis::bundled_godot_version()`
   (+ optionally `engine_version()`), so the matrix is observable through the API, not just docs.
4. **Enforcement:** `cargo-semver-checks` as a **PR gate** *and* the release gate; a
   `#[non_exhaustive]` audit test; an FFI-JSON-schema round-trip test.
5. **An API-review pass** (§3.6 checklist) signed off and recorded as an ADR.

---

## 3. Design — the concrete edits

### 3.1 Add `#[non_exhaustive]` (the core edit)

Apply to the enums + result structs marked **YES** in §1.2 and to `Change` (§1.4). Idiom matches the
plan §6.1 sketch but uses the **real** field/variant names. Examples (note `Info`, not `Information`;
note `code: String` until §3.4 lands the typed enum):

```rust
// crates/gdscript-base/src/lib.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]                         // a new severity is additive
pub enum Severity { Error, Warning, Info, Hint }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]                         // new fields (tags, related) become minor
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: Severity,
    pub code: String,                     // (W1: → WarningCode; freeze the type here, §3.4)
    pub message: String,
    #[serde(default)] pub source: DiagnosticSource,
    #[serde(default)] pub fixes: Vec<CodeAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]                         // SemanticTokenType is the most likely to grow
pub enum SemanticTokenType { Function, Method, /* … */ Constant }
```

```rust
// crates/gdscript-ide/src/lib.rs
#[derive(Debug, Default)]
#[non_exhaustive]                         // force the builders to be the stable construction path
pub struct Change { pub files: /*…*/, pub paths: /*…*/, pub project_config: /*…*/ }
```

**Mechanical consequence inside the workspace:** `#[non_exhaustive]` blocks struct-literal
construction and non-`_`-terminated matches **from other crates**, but *not* within the defining
crate. The features in `gdscript-ide`/`gdscript-hir` that *build* these PODs are in different crates,
so they must construct via constructors. Two options, decide in M0:
- (a) Add `pub fn new(...)` constructors to each result struct (rust-analyzer style), or
- (b) keep construction inside `gdscript-base` behind `pub(crate)`-free builders and have producers
  call them.
Recommendation: **(a)** — small `::new` constructors. They are cheap, document the required fields,
and are the same pattern the playbook already uses (`TextRange::new`, `SourceChange::single`).
Enums matched internally (`Severity`, `SemanticTokenType`, …) need no change to *produce*; only
*external* exhaustive matches break, which is the intended forcing function.

### 3.2 Wire `cargo-semver-checks` as a PR gate

Add a job to `.github/workflows/ci.yml` (it is absent today). `cargo-semver-checks` only inspects
**published** crates, so it naturally covers exactly the 7 `publish = true` crates:

```yaml
  semver:
    name: semver-checks
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: obi1kenobi/cargo-semver-checks-action@v2
        with:
          # Compare the PR's public API against the last released version on crates.io.
          # Fails the PR on an UNMARKED break (a marked breaking change is allowed but
          # flagged for the bump). Pin the action by SHA per the repo's supply-chain policy
          # (see release-plz.yml's note on SHA-pinning).
          rust-toolchain: stable
```

This makes the break visible **on the PR** (where it can be discussed/labeled `breaking`) instead of
only when release-plz assembles the Release PR. Keep `semver_check = true` in `release-plz.toml` as
the belt-and-suspenders release gate.

### 3.3 Scope the contract correctly (the §0.2 correction)

The policy text (§4) and `gdscript-ide`'s crate rustdoc must name **`gdscript-ide` + `gdscript-base`
+ the `@gdscript-analyzer/*` POD JSON** as the stable surface — because `gdscript-base` is a
separately-published crate every consumer imports from. Two acceptable framings:

- **Pragmatic (1.0):** declare *both* `gdscript-ide` and `gdscript-base` stable. Simplest; honest to
  the code as it stands. `cargo-semver-checks` already guards both.
- **Façade (post-1.0, a 2.0-era refactor, do NOT do for 1.0):** re-export every POD from
  `gdscript-ide` (`pub use gdscript_base::{…};`) and document `gdscript-base` as internal. This would
  let `gdscript-base` churn — but re-exporting now and *then* trying to hide `gdscript-base` is itself
  a consumer-visible move. Note it in the post-1.0 roadmap; ship the pragmatic framing at 1.0.

### 3.4 Coordinate with W1 on `Diagnostic.code`

W1 introduces a `WarningCode` enum (`#[non_exhaustive]`). **`Diagnostic.code`'s type is a contract
field.** If W1 lands *after* the 1.0 cut, changing `code: String → code: WarningCode` is a **major**
break. Therefore:
- **Decision to make in M0:** either (a) land W1's `WarningCode` and flip `Diagnostic.code` to it
  **before** the freeze, or (b) keep `code: String` at 1.0 and have `WarningCode` be a *helper* that
  parses/formats the string (so the field type never changes). The plan's §6.1 sketch assumes (a);
  the *code* ships (b) today. **Pick one and freeze it.** Recommendation: **(a)** if W1 is on track
  for the same release (cleaner, what the plan intends); otherwise **(b)** with a documented
  `WarningCode::as_str()`/`from_code()` and the field staying `String` — never a mid-1.x type change.

### 3.5 Surface the Godot version on the public API (new — does not exist)

The matrix in §5 must be **observable**, not just prose. Add to `Analysis`:

```rust
// crates/gdscript-ide/src/lib.rs
impl Analysis {
    /// The Godot API version this analyzer's *bundled* engine model was generated from
    /// (e.g. "4.5.stable"). Stable across the session. Drives the supported-version matrix.
    #[must_use]
    pub fn bundled_godot_version(&self) -> &'static str {
        gdscript_api::godot_version()           // already exists at gdscript-api/src/lib.rs:38
    }

    /// The project's *declared* engine `(major, minor)` from `project.godot`
    /// (`[application] config/features`), if a project config was supplied. The analyzer snaps
    /// this to the nearest bundled minor for per-project API selection.
    ///
    /// # Errors
    /// `Err(Cancelled)` if a concurrent `apply_change` invalidated this snapshot.
    pub fn project_engine_version(&self) -> Cancellable<Option<(u32, u32)>> {
        catch(|| gdscript_hir::project_engine_version(&self.db))   // exists at hir/queries.rs:199
    }
}
```

Both wrap **existing internal queries** — this is pure surfacing, not new analysis. `(u32, u32)` is a
fine POD return (closed tuple). This is the only **new public method** the workstream adds, and it is
what ties the §5 matrix to something a consumer can read.

### 3.6 The API-review checklist (run once, record as an ADR)

A one-pass review of the frozen surface. Each item is *verified against the code*, not assumed:

- [ ] **No `lsp-types`/protocol types leak** into `gdscript-base`/`gdscript-ide`. *(Verified clean
  today; re-check after any edit.)*
- [ ] **All positions are byte offsets** (`u32` over UTF-8); UTF-16 conversion is the client's job
  via `LineIndex`. *(Verified: every range type is byte-based.)*
- [ ] **`Cancellable<T>` on every read query.** *(Verified: all 15.)*
- [ ] **POD is serde-round-trippable** and matches the documented JSON schema (§3.7 test).
- [ ] **`#[non_exhaustive]` audit:** every enum/result-struct a consumer matches carries it; the
  constructible value types deliberately do not (documented why).
- [ ] **Naming consistency:** `Info` (not `Information`); `goto_definition` not `go_to_definition`;
  `*_hints`/`*_ranges` plural for `Vec` returns. Freeze the spellings.
- [ ] **No accidental `pub`:** `catch` is private; feature modules are private; `db` fields private.
  *(Verified.)*
- [ ] **`Debug` on every public type** (workspace lint `missing_debug_implementations` enforces it).
- [ ] **`#[doc]` on every public item** (rustdoc; W5 polishes — this review just checks coverage).
- [ ] **The `Change` builder is the construction path** (`#[non_exhaustive]` set; §1.4).
- [ ] **MSRV documented** (`1.88.0`, workspace) and its bump policy = minor.

Record the outcome as `docs/src/adr/0004-1.0-api-freeze.md`.

### 3.7 The FFI JSON schema (document + test)

Write the schema as `docs/src/reference/json-schema.md` (or a JSON Schema file) covering: each POD's
field names/types (from the serde derives — `rename_all` matters: `Severity` is `"lowercase"`,
`SemanticTokenType` is `"camelCase"`, `RenameError` is adjacently tagged `{"kind": …}`) **plus the
rename envelope** `{"ok"|"error"}`. The schema is the **npm consumers' contract**; the Rust structs
are the **crates.io consumers' contract** — and they must not diverge. The round-trip test (§5
Testing) asserts a serialized POD matches the schema and deserializes back equal.

---

## 4. The semver / stability policy (verbatim — for the contract page + crate rustdoc)

> **`gdscript-analyzer` 1.0 stability policy.** From `1.0.0` we follow **SemVer 2.0.0** (replacing the
> 0.x Cargo reading used through `0.x`). The **stable public API** is: the **`gdscript-ide`** crate's
> public surface (`AnalysisHost`, `Change`, `Analysis` and its query methods), the **`gdscript-base`**
> crate's public POD result types (which `gdscript-ide`'s signatures return and which consumers import
> directly), and the **`@gdscript-analyzer/core` (napi) + `@gdscript-analyzer/wasm`** packages' POD
> **JSON** result shapes (one shared version across crates.io and npm — the JSON is the serde
> projection of the `gdscript-base` types plus the documented `rename` envelope). **MAJOR** = a
> breaking change to that surface: removing or renaming a method/type/field/variant, changing a
> field's or return type, narrowing accepted input, or removing `#[non_exhaustive]`. **MINOR** =
> additive, backward-compatible: a new method, a new `#[non_exhaustive]` enum variant or struct field,
> a new diagnostic code, a new config option, a new optional JSON field. **PATCH** = a bug/behavior
> fix with no surface change.
>
> **Explicitly NOT covered by this guarantee:** the internal crates **`gdscript-hir`,
> `gdscript-db`, `gdscript-syntax`, `gdscript-api`, `gdscript-scene`** — they are published to
> crates.io only as build dependencies of `gdscript-ide` and **may change freely**; do not depend on
> them directly. The **exact wording of diagnostic messages** (we track Godot's strings, which change
> between engine versions — the stable identifier is the diagnostic **`code`**, never the message
> text). **Inference precision** — a value previously typed `Variant` becoming a concrete type, or an
> `UNSAFE_*` warning that previously fired no longer firing because narrowing improved — is a
> **quality** change shipped in MINOR/PATCH, **not** a break. The `LineIndex` internals.
>
> **Deprecation policy:** a stable item is marked `#[deprecated]` (with a `note` pointing to its
> replacement) for **≥1 minor release** before removal in the next major. **MSRV** bumps are
> **minor** and noted in the changelog (current MSRV: `1.88.0`). **Enforcement:**
> `cargo-semver-checks` runs on every PR and at release; an unmarked break to a stable crate fails CI.

> **Scope note (honest, code-grounded):** at 1.0 the stable POD lives in the separately-published
> `gdscript-base` crate, so `gdscript-base` is stable *too*. A future major may re-export the POD
> through `gdscript-ide` and privatize `gdscript-base`; until then, both are contract.

---

## 5. The supported-Godot-version matrix (verbatim — for the contract page)

The analyzer bundles several Godot minors' API and selects per project (detected from
`project.godot` `[application] config/features`, snapped to the nearest bundled minor, newest as
default, overridable — `01-ARCHITECTURE.md` §5; the detection query already ships at
`gdscript-hir/src/queries.rs:192`). The bundled version is observable via
`Analysis::bundled_godot_version()` (§3.5).

| Analyzer | Bundled Godot APIs | Default | Notes |
|---|---|---|---|
| `1.0.x` | 4.3, 4.4, 4.5, …newest stable at cut | newest | 4.3 = oldest supported; master-only warnings (W1 `WarningCode::since`) gated off for 4.3. |
| policy | a new Godot **minor** → added in a **minor** analyzer release (additive, via the Godot-sync PR — [`GODOT-SYNC.md`](GODOT-SYNC.md)) | newest | dropping an old Godot minor → a **major** analyzer release. |

> **Policy statement.** We support the latest **N Godot minors (N ≥ 3)**. A new Godot stable minor is
> picked up automatically by the Godot-sync workflow and shipped in the **next minor** analyzer
> release (additive — a new bundled API never breaks consumers). **Dropping** support for a Godot
> minor is a **breaking change** (major analyzer bump), because a project pinned to it loses its
> nearest-minor target. Per-project selection is detected from `project.godot`; absent a project
> config, the **newest** bundled API is used.

(Fill the exact bundled-version list from `vendor/godot/` + `gdscript_api::godot_version()` at the
cut — do not hardcode "4.5" in code; read it from the API.)

---

## 6. Step-by-step implementation plan

Each step ends green through `cargo xtask ci` (incl. the wasm32 portability check) + the new `semver`
job, matching the prior playbooks' milestone discipline.

**M0 — the freeze prep (decisions + scaffolding).**
1. Decide the two open questions: (a) `Diagnostic.code` type — flip to `WarningCode` with W1, or keep
   `String` + helper (§3.4); (b) constructor strategy for `#[non_exhaustive]` PODs — add `::new`
   constructors vs internal builders (§3.1). Record both in `adr/0004`.
2. Add the **`semver` PR job** (§3.2) — it will currently pass (no break vs `0.2.1`); it exists to
   catch the *next* break and to validate wiring before the surface is frozen.

**M1 — `#[non_exhaustive]` + constructors.**
3. Add `#[non_exhaustive]` to the §1.2-YES types + `Change` (§3.1).
4. Add the `::new` constructors (or builders) so in-workspace producers compile; fix the producer
   call sites in `gdscript-hir`/`gdscript-ide` features.
5. `cargo xtask ci` green; the `semver` job now reports these as **breaking vs `0.2.1`** — expected,
   because adding `#[non_exhaustive]` *is* a break. **This is the last allowed pre-1.0 break**; it
   rides the `0.2.1 → 0.3.0` (or the 1.0) bump. Label the PR `breaking`.

**M2 — surface the Godot version (§3.5).**
6. Add `Analysis::bundled_godot_version()` + `Analysis::project_engine_version()` wrapping the
   existing internal queries. Add a unit test (mirror the existing `lib.rs` tests' `host_with`
   pattern) asserting the version is non-empty and the project version round-trips.

**M3 — the contract, docs, schema, tests.**
7. Write the **policy** (§4) into `gdscript-ide`'s crate rustdoc **and**
   `docs/src/reference/contract.md`; add it to `SUMMARY.md` under **Reference** (the section exists in
   `docs/src/SUMMARY.md` per W5's skeleton — add `[Stability & supported versions](reference/contract.md)`).
8. Write the **Godot matrix** (§5) into the same contract page, reading bundled versions from the API.
9. Write the **FFI JSON schema** (§3.7) → `docs/src/reference/json-schema.md`.
10. Add the enforcement tests (§7).

**M4 — the API-review pass + sign-off.**
11. Walk the §3.6 checklist against the final surface; fix any naming/`pub` slip found.
12. Finalize `adr/0004-1.0-api-freeze.md` recording the frozen surface + the two M0 decisions.
13. Per-milestone **adversarial review** (find→verify→fix) as every prior milestone did.

---

## 7. Test plan

1. **`#[non_exhaustive]` audit test** (compile-time, in `gdscript-base`): a `tests/non_exhaustive.rs`
   integration test (a *separate crate*, so it sees the external view) that attempts an exhaustive
   match / struct literal on each marked type behind `#[cfg(...)]` and asserts — via a doc-comment
   contract or a `trybuild` UI test — that it does **not** compile without a `_` arm / `..`. Simpler
   acceptable form: a `trybuild` `compile_fail` case per category (one enum, one struct), proving the
   forcing function works. This is the "every consumer-matched type carries it" check from the plan's
   Testing #7.
2. **FFI-JSON-schema round-trip** (`gdscript-session` tests, extending the existing ones at
   `lib.rs:318+`): serialize a representative of **each** POD + the rename envelope, assert it
   matches the documented schema (field names honoring `rename_all`), deserialize back, assert equal.
   The existing tests already check `diags[0]["code"] == "INTEGER_DIVISION"` and the
   `{"ok"|"error"}` envelope — extend to cover field-name stability for every type. This guards
   "npm and Rust contracts can't diverge" (Testing #7).
3. **`cargo-semver-checks` differential** (the new CI job, §3.2): on every PR, compares the PR surface
   to the last crates.io release; a break without the `breaking` label fails. After 1.0, this is the
   2.0-tripwire.
4. **Public-API doctest** (in `gdscript-ide` crate rustdoc): a top-level `AnalysisHost`→`Analysis`→
   `diagnostics` usage example, `cargo test --doc` green (also W5's docs.rs polish item). This is the
   "doctest of the documented public-API usage" from Testing #7.
5. **Godot-version surface test** (M2): asserts `bundled_godot_version()` is non-empty and
   `project_engine_version()` returns `Some((4, n))` for a fixture `project.godot` and `None` with no
   config — mirrors `gdscript-hir/queries.rs:1410`'s firewall test at the public boundary.
6. **`cargo public-api` snapshot (optional, deferred):** a golden snapshot of the full `gdscript-ide`
   + `gdscript-base` surface, blessed like the `expect-test` goldens, so *any* surface delta shows in
   review. `cargo-semver-checks` is the gate; this is human-readable defense-in-depth. Mark as
   nice-to-have in TECH_DEBT, not a 1.0 blocker.

---

## 8. Risks & mitigations

| # | Risk | Sev | Mitigation |
|---|---|---|---|
| R1 | **The "internals are free to change" promise is false as built.** All 7 core crates are `publish = true`, so `cargo-semver-checks` gates `gdscript-hir/-db/-syntax/-api/-scene` too — a refactor there blocks release. | **High (process)** | Two honest options, decide at M0: **(a)** accept it — these crates *are* on crates.io, so semver-checking them is correct; the policy says "don't depend on them" but CI still guards them (extra safety, mild friction). **(b)** mark the truly-internal crates `publish = false` and have `gdscript-ide` vendor/inline what it needs — larger change, defer to post-1.0. **Recommend (a)**; document that internal-crate breaks ride a workspace minor and are *not* a consumer-facing break despite the version bump. |
| R2 | **1.0 lock-in regret** — freezing the wrong shape forces a premature 2.0. | High | The explicit **API-review pass** (§3.6) before the cut; `#[non_exhaustive]` on every growable type so additive change stays minor; the contract scoped tightly (excludes internals/messages/precision); **two real consumers already exercise the surface** (the standalone LSP `gdscript-lsp` and the guitkx napi path) *before* the freeze. |
| R3 | **`#[non_exhaustive]` itself is a breaking change** — adding it post-1.0 is a major bump. | **High (timing)** | Land **all** of §3.1 in the **last pre-1.0 release** (M1), riding the existing `0.x` "breaking = minor" reading; after that the surface only grows additively. Missing one type means a forced early 2.0. The audit test (§7.1) catches a missed type *before* the cut. |
| R4 | **`Diagnostic.code` type changes mid-1.x** (String→WarningCode) — a silent major break. | Med | Freeze the type **once** at the cut (§3.4); coordinate with W1's milestone; if W1 slips, ship `code: String` + a `WarningCode` *helper*, never a later type flip. |
| R5 | **JSON drift between Rust and npm** — a serde rename or a non-`#[serde(default)]` new field breaks deserialization for old npm clients. | Med | Every new field is `#[serde(default)]` (the `Diagnostic.source`/`fixes` precedent at `gdscript-base:84-88`); the schema round-trip test (§7.2) + the documented schema (§3.7) are the contract; `rename_all` values are frozen in the review. |
| R6 | **`cargo-semver-checks` misses an FFI-only break** (it checks Rust APIs, not JSON shapes). | Med | The JSON shape is a *serde projection* of the checked Rust types, so most breaks surface in Rust; the residual (a `rename_all` change, the rename envelope) is covered by the §7.2 schema test, which `cargo-semver-checks` cannot see. |
| R7 | **The supported-version matrix is prose-only / drifts from what's bundled.** | Low | `Analysis::bundled_godot_version()` (§3.5) makes it observable; the docs page reads the version from `gdscript_api::godot_version()` rather than hardcoding it; the Godot-sync PR updates both the bundle and the matrix together. |

**Biggest risk:** R3 — the timing of the `#[non_exhaustive]` edit; it *must* be the last pre-1.0
break. **Biggest leverage:** the surface is already the right shape and `Cancellable`/byte-offset/no-
lsp-types are already satisfied — this workstream is a freeze + a handful of attributes + a CI job +
docs, not a redesign.

---

## 9. Dependencies on other workstreams

- **W1 (warning set):** owns `WarningCode`. **Hard coupling** on `Diagnostic.code`'s type (§3.4) — the
  type must be frozen jointly at the cut. W1's `#[non_exhaustive] enum WarningCode` is part of this
  workstream's audit.
- **W5 (docs):** the **contract page** (policy §4 + matrix §5) and the **FFI JSON schema** (§3.7) live
  in the mdBook W5 finishes; W5's `SUMMARY.md` already has a **Reference** section to slot them into.
  docs.rs polish (W5) and this workstream's rustdoc coverage check (§3.6) overlap — coordinate so the
  policy text lives in *both* the book and the crate rustdoc.
- **W3 (formatter):** if `Analysis::format` is added (plan §2 sketch lists it; it is **not** in the
  code today), it joins the frozen read surface and must be `Cancellable` + return `SourceChange`
  POD — review it under §3.6 before the cut. If W3 slips past 1.0, `format` is added in a later minor
  (additive, fine).
- **W4 (perf):** no API coupling, but the `bench` baseline is taken against the frozen surface; don't
  reshape the API after the baseline.
- **W7 (governance):** the API-review ADR (`adr/0004`) and the deprecation policy feed W7's RFC/ADR
  process; the "≥1 external consumer" criterion is exercised *against this frozen surface*, so the
  freeze should precede the outreach push.
- **Release tooling (Phase 0):** `release-plz.toml`'s `semver_check = true` is the release gate; this
  workstream **adds the missing PR gate** (§3.2). The single shared workspace version + single `v{}`
  tag (release-plz.toml) already implement "one version across crates.io + npm" — verified, no change.

---

## 10. Sources (grounded in the code, 2026)

- **Code inventoried:** `crates/gdscript-ide/src/lib.rs`, `crates/gdscript-base/src/lib.rs`,
  `crates/gdscript-session/src/lib.rs`, `crates/gdscript-ffi/src/lib.rs`, `bindings/wasm/src/lib.rs`,
  `crates/gdscript-api/src/lib.rs` (`godot_version`), `crates/gdscript-hir/src/queries.rs`
  (`engine_version`/`project_engine_version`), `Cargo.toml`, `release-plz.toml`,
  `.github/workflows/{release-plz,ci}.yml`, `docs/src/SUMMARY.md`, `TECH_DEBT.md`.
- **Plan/architecture:** `PHASE-6-V1-RELEASE.md` §"What 1.0 commits us to" + §Workstream 6;
  `01-ARCHITECTURE.md` §2 (API rules) + §9 (SemVer row); `research/07-ecosystem-and-release-tooling.md`
  §3.3 (0.x→1.0 reading, the `cargo-semver-checks` adoption).
- **External:** SemVer 2.0.0; the Cargo SemVer reference (`doc.rust-lang.org/cargo/reference/semver.html`,
  incl. `#[non_exhaustive]` as a minor-additive enabler); `cargo-semver-checks` + its GitHub Action;
  `cargo public-api`; rust-analyzer's `ide`/`ide-db` POD-surface split.
