# Phase 6 · Workstream 4 — Performance Hardening Playbook

> Research-backed, **code-grounded** build plan for the 1.0 performance-hardening workstream:
> the tiered real-Godot-game benchmark fixture, the criterion harness + metrics table, memory
> profiling (dhat), salsa cache tuning + LRU eviction, the wasm bundle-size budget, and the CI
> bench/wasm-size regression guards. Matches the Phase-5 playbook format and depth.
>
> **Parent docs:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 4,
> [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §3 (salsa/durability), §4 (FFI/WASM bundle), §5 (data
> model / multi-version), §7 (portability rules); evidence:
> [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md) §4 (the size budget),
> [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) (salsa), and the
> Phase-3 firewall implemented in `crates/gdscript-hir/src/queries.rs`.
>
> **Verification stance:** this playbook was written against the **actual tree at
> `cleanup_and_upgrades`**. Where the canonical plan describes infrastructure that does **not yet
> exist** (a `fixtures/perf/` corpus, a brotli engine-asset path, LOW/MEDIUM/**HIGH** durability, a
> bench CI job), it is corrected inline and flagged **⚠ PLAN DIVERGES**.

---

## 0. The one-line thesis

The incremental *machinery* is already built and already **firewall-tested** — the salsa graph in
`crates/gdscript-db/src/lib.rs`, the durability split, and the re-execution-counting tests in
`crates/gdscript-hir/src/queries.rs` (`WITNESS_RUNS`, `REGISTRY_OBSERVED`). What is missing is
**measurement at scale and guards against regression**: there is exactly **one** bench today
(`crates/gdscript-ide/benches/analysis.rs`, single ~300-line file, Phase-2 era), **no** project-scale
fixture (`fixtures/` holds only empty `ide/.gitkeep` + `parser/.gitkeep`), **no** memory profiling,
**no** salsa LRU eviction, **no** brotli/content-hashed engine asset, and **no** bench-or-wasm-size CI
job. This workstream turns the existing-but-unmeasured incremental promise into **numbers with a
ceiling CI enforces.** Build the fixture first; everything else hangs off it.

---

## 1. Goal — the 1.0 cut vs the deferred tail

### The 1.0 cut (what ships, all measured + guarded)

1. A **tiered real-Godot-game fixture** vendored under `fixtures/perf/` (small ≈50 files, medium
   ≈300, large ≈1000+), license-compatible, exercising deep `class_name`/`extends`, autoloads, and
   scene-heavy `.tscn` node-path typing.
2. A **criterion harness** at the workspace level (a new `benches/` member or `gdscript-ide` benches)
   measuring the §4.2 metrics table: cold full-project analyze, **warm keystroke < 10 ms** (flat as
   the project grows), **bounded invalidation** on signature/`class_name` edits (asserted by
   query-recount, not just wall-clock), **member completion < 5 ms** warm, and **parse MB/s**.
3. **Memory profiling** via `dhat` on the large fixture: a documented resident-state ceiling, proof
   the engine model is loaded **once** (it is — `OnceLock` in `gdscript-api`), and small `Ty`.
4. **Salsa LRU eviction** for cold-file derived data so a 1000+-file project's resident set is
   bounded (rust-analyzer's approach).
5. The **wasm bundle-size budget**: the `wasm-opt -Oz` pass (already configured) measured + ceiling'd,
   **plus** the currently-missing brotli + content-hashed engine-asset path (`set_engine_api` /
   `load_engine_api` exist; the *asset pipeline* does not).
6. **CI regression guards:** a `bench` job that fails a PR on **> 10 %** regression of any tracked
   metric, and a **wasm-size guard** that fails over a hard byte ceiling. The existing wasm32
   portability guard (`ci.yml` job `wasm`) stays green.

### The deferred tail (explicit non-goals at 1.0)

| Deferred | Why |
|---|---|
| **Parallelism for throughput** | Feature-gated, native-only, post-1.0 ([`01`](01-ARCHITECTURE.md) §7 rule 3). `rayon` is already a dep for the CLI's *file fan-out*, but per-query parallelism inside the salsa graph is out. |
| **Zero-copy archived engine access** (`rkyv::access` instead of `from_bytes`) | `EngineApi::from_bytes` currently does a **full eager decode** into owned `ApiData`. Switching to zero-copy archived reads is a real win but a larger refactor of `gdscript-api` — measure the cost at 1.0, defer the rewrite if the budget holds. |
| **Sub-millisecond cold start on huge projects** | 1.0 sets the cold budget *from first measurement* and guards ±10 %; chasing an absolute target is post-1.0. |
| **`full/delta` semantic-token streaming**, incremental reparse below the file granularity | Post-1.0; parse is already fast enough per the existing bench. |

---

## 2. Current state — what EXISTS vs the gap

### 2.1 What exists today (real paths + symbols)

**The salsa graph + durability (`crates/gdscript-db/src/lib.rs`).** ⚠ **PLAN DIVERGES:** the plan and
§4.3 of the parent say *"LOW/MEDIUM/HIGH durability."* The code uses **only two tiers**:

- **`Durability::LOW`** — `FileText::text` (the edited file; set in `AnalysisHost::apply_change`,
  `lib.rs:111`, `set_file_text(... Durability::LOW)`).
- **`Durability::MEDIUM`** — `FileText::res_path` (`set_file_path`, `db lib.rs:153`), `SourceRoot`
  (`sync_source_root`, `db lib.rs:299/306`), `ProjectConfig` (`set_project_config`,
  `db lib.rs:240/245`), and (wasm-only) `EngineGeneration` (`db lib.rs:275/280`).
- **There is no `Durability::HIGH` anywhere** (`grep -rn "Durability::HIGH" crates/` → empty). The
  engine model is **not** a salsa input at all — it is a leaked `&'static EngineApi`
  (`RootDatabase::engine: Option<&'static EngineApi>`, `db lib.rs:193`) backed by an `OnceLock` on
  native (`gdscript-api/src/lib.rs:161-168 bundled()`). So the "durable Godot stdlib API" the plan
  describes is durable by being *outside salsa entirely*, not by `Durability::HIGH`.
  **The playbook should either (a) document the two-tier reality, or (b) promote the engine model to a
  `Durability::HIGH` salsa input if perf work needs salsa to track it. Recommendation: keep it outside
  salsa (zero overhead on native) and correct the docs — see §6.1.**

**The body-edit firewall + the query-recount infrastructure** (`crates/gdscript-hir/src/queries.rs`,
the `#[cfg(test)] mod tests` block, lines ≈450-570). This is the existing infra the parent plan's
§4.3 calls *"a test that counts query re-executions"* — it already exists:

- `static WITNESS_RUNS: AtomicU32` + `#[salsa::tracked] fn class_name_witness` (depends **only** on
  `item_tree`, bumps the counter on each real execution) → test
  `body_edit_does_not_invalidate_signature_queries` (`queries.rs:471`): warms the cache, edits **only
  a body keeping byte length**, asserts `WITNESS_RUNS` is **unchanged** (the `item_tree` backdate
  firewall holds).
- `static REGISTRY_OBSERVED: AtomicU32` + `#[salsa::tracked] fn observe_registry` → test
  `body_edit_does_not_invalidate_the_global_registry` (`queries.rs:540`): a **length-changing** body
  edit must not re-run a `global_registry` consumer.
- Two further counters: `RES_REGISTRY_OBSERVED` (`queries.rs:920`) and `AUTOLOAD_OBSERVED`
  (`queries.rs:1512`) — the same idiom for the resource-path and autoload registries.

**This is the load-bearing prior art for the entire bounded-invalidation metric.** The perf benches
do **not** need to invent recount infrastructure; they need to **lift this idiom out of `#[cfg(test)]`
into a reusable harness** (a salsa event/`LogDatabase` recorder or an exported counting-witness query)
that a criterion bench and a CI assertion can both call. See §3.2.

**The benches (`crates/gdscript-ide/benches/analysis.rs`).** One criterion bench, registered in
`crates/gdscript-ide/Cargo.toml` (`[[bench]] name = "analysis" harness = false`,
`criterion = { workspace = true }` dev-dep). It builds a **single ~300-line synthetic** `.gd`
(`sample_script()`, 36 generated methods), warms the engine model, then benches `diagnostics`,
`hover`, `member_completion`. Its own header notes *"There is no salsa cache in Phase 2"* — it is a
**Phase-2 single-file** bench. It does **not** measure: warm keystroke incremental re-query, bounded
invalidation, project-scale cold analyze, or parse throughput. `criterion = "0.5"` is the workspace
dep (`Cargo.toml:126`).

**The corpus runner (`crates/gdscript-ide/examples/corpus.rs`).** An ad-hoc, **untimed** correctness
runner: `cargo run -p gdscript-ide --example corpus -- <dir> [--show] [--project]`. `--project` loads
every `.gd` into **one** `AnalysisHost` (populating the global `class_name` registry + autoloads via
`set_project_config`), runs `diagnostics(file)` per file under `catch_unwind`, and reports
files/clean/with-diags/panics. **This is the natural seed for the project-scale *cold-analyze*
bench** — it already walks a directory, sets `res://` paths, and drives the full salsa pipeline; the
bench is "this loop, but timed, against `fixtures/perf/`."

**The public API the benches drive** (`crates/gdscript-ide/src/lib.rs`): `AnalysisHost::new()` /
`::apply_change(Change)` (the **only** mutation entry point; `lib.rs:105`) / `::analysis() -> Analysis`
(a cheap cloned salsa handle) / `::set_engine_api(&[u8]) -> bool`. `Change` carries
`files`/`paths`/`project_config`. Every read query returns `Cancellable<T>` (a salsa-cancellation
guard). `Session` (`crates/gdscript-session/src/lib.rs`) wraps this for the bindings with a
URI→`FileId` interner and `open`/`change`/`close`/`set_project_config`/`load_engine_api`.

**The engine model (`crates/gdscript-api`).** `engine_api.bin` is a **3.47 MB raw rkyv blob**
(`crates/gdscript-api/src/engine_api.bin`). `bundled()` decodes it **once** via `OnceLock`
(native, `include_bytes!`). `from_bytes(&[u8])` copies into a 16-byte-`AlignedVec` and **fully
deserializes** into owned `ApiData` + rebuilds the name→id `FxHashMap` indices (`lib.rs:84-140`).
On `wasm32` the blob is **not** embedded; the page must `fetch` it and call
`load_engine_api`/`set_engine_api`. **This eager decode + the 3.47 MB asset are the two biggest
memory/bundle line items.**

**The wasm pipeline.** `bindings/wasm` (`gdscript-wasm`, `crate-type = ["cdylib","rlib"]`) wraps
`Session`. Its `Cargo.toml` already sets `[package.metadata.wasm-pack.profile.release] wasm-opt =
["-Oz"]` — so `wasm-pack build --release` **does** run `wasm-opt -Oz`. The root `Cargo.toml` also has
a `[profile.wasm-release]` (`opt-level="z"`, `lto`, `panic="abort"`, `strip`) — but **note** wasm-pack
`--release` uses the *shared `release` profile*, **not** `wasm-release` (wasm-pack can't select a
custom profile), so `panic="abort"`/`strip` from `wasm-release` are **not** applied to the published
bundle today (the `wasm-pack` `wasm-opt` pass is the only size lever actually consumed — its own crate
comment says so). The current built artifact is **≈777 KB** (`playground/pkg/gdscript_bg.wasm`),
already under the 1.5 MB budget **before brotli**.

**CI (`.github/workflows/ci.yml`).** Jobs: `fmt`, `clippy`, `test` (3-OS), `msrv` (1.88.0), `wasm`
(portability guard: `cargo check -p gdscript-ide` + `cargo build -p gdscript-wasm` for
wasm32-unknown-unknown, plus a `getrandom`-stays-out assertion), `differential`, `bindings`,
`coverage`, `deny`, `pr-title`. `xtask ci` (`xtask/src/tasks.rs`) mirrors fmt/clippy/test/wasm-check/
deny. **No `bench` job, no wasm-size job exist.** `release-wasm.yml` builds with `wasm-pack build
--target web --release` and publishes — **no brotli step, no size gate.**

### 2.2 The gap (precise)

| Need | Status | Where it must land |
|---|---|---|
| Tiered real-game perf fixture | **MISSING** (`fixtures/` is empty placeholders) | `fixtures/perf/{small,medium,large}/` |
| Project-scale cold-analyze bench | **MISSING** (only single-file Phase-2 bench) | new bench, seeded from `examples/corpus.rs` |
| Warm-keystroke incremental bench | **MISSING** | new bench: `apply_change` a body edit, re-`diagnostics` |
| Bounded-invalidation metric (recount) | **infra exists in `#[cfg(test)]`** | lift `WITNESS_RUNS` idiom into a shared harness |
| Parse MB/s bench | **MISSING** | bench over `gdscript_syntax::parse` on the fixture |
| Memory profiling (dhat) | **MISSING** (dhat not a dep) | `dhat` dev-dep + a `--features dhat` example/bin |
| Salsa LRU eviction | **MISSING** | `gdscript-db` (salsa 0.27 LRU API) |
| Brotli + content-hashed engine asset | **MISSING** (`-Oz` present; brotli/asset absent) | `release-wasm.yml` + an xtask `wasm-dist` task |
| Bench CI guard (>10 % fails) | **MISSING** | new `.github/workflows/` job or `ci.yml` job |
| Wasm-size CI guard (byte ceiling) | **MISSING** | `ci.yml` / `release-wasm.yml` step |

---

## 3. Design — concrete fixtures, harness, metrics, memory, eviction, bundle

### 3.1 The tiered real-Godot-game fixture (`fixtures/perf/`)

**Layout** (a vendored, license-compatible real Godot 4.x project, tiered by sub-selection or by three
separate projects):

```
fixtures/perf/
  README.md            # provenance, license (MIT/CC0 — record the SOURCE.txt like vendor/godot/),
                       # the commit SHA vendored, and "regenerate" instructions
  small/   project.godot + ~50 .gd + .tscn        # microbench tier; fast PR signal
  medium/  project.godot + ~300 .gd + .tscn        # the "typical game" tier
  large/   project.godot + ~1000+ .gd + .tscn      # the stress tier; LRU + memory tier
  manifest.toml        # per-tier: file_count, total_bytes, the "hot" file + body-edit span,
                       # the signature-edit span, the class_name-edit file, a completion site
```

**Selection criteria** (must exercise the whole stack): many `.gd`, a deep `class_name`/`extends`
graph, **autoloads** (so `ProjectConfig` is live), and **scene-heavy** `.tscn` node-path typing
(Phase 4 — `$Path`/`%Unique`/`get_node`). Candidates: a **Maaack menu-template-based** project, the
**Godot demo-projects** superset, or a vendored open-source game (e.g. an MIT-licensed jam game).
**License gate:** MIT/Apache/CC0/BSD only; record provenance in `fixtures/perf/README.md` exactly like
`vendor/godot/4.5-stable/SOURCE.txt`. Keep the fixture **deterministic** (a pinned commit), since
golden timings depend on stable inputs.

**The `manifest.toml`** is the key to making benches non-brittle: instead of hard-coding byte offsets
(the Phase-2 bench does `src.find("b.x")` — fine for a synthetic file, fragile for 1000 real files),
the manifest names, per tier, the file + span the bench should edit and the position to complete at.
Loaded once at bench setup.

```toml
# fixtures/perf/manifest.toml  (sketch)
[medium]
root = "medium"
file_count = 300
total_bytes = 1_200_000
hot_file = "res://player/player.gd"     # the file the warm-keystroke bench edits
body_edit = { marker = "# BENCH_BODY_EDIT", insert = "\tvar _bench := 1\n" }  # in a func body
signature_edit = { file = "res://player/player.gd", marker = "# BENCH_SIG_EDIT" } # adds a param
class_name_edit = { file = "res://enemy/enemy.gd" }   # renames the class_name → recount target
completion_site = { file = "res://player/player.gd", marker = "# BENCH_COMPLETE" } # a `recv.` site
```

(Markers are comments embedded in the vendored sources so the bench locates spans by `find(marker)`
rather than a brittle byte offset — robust across re-vendoring.)

### 3.2 The criterion harness (a new top-level `benches/` member)

⚠ The repo-root `benches/` dir holds only `.gitkeep` and is **not a crate** — `criterion` lives in
`gdscript-ide`'s dev-deps. Two viable homes:

- **(A) Extend `crates/gdscript-ide/benches/`** with a new `project.rs` bench (simplest; reuses the
  existing `[[bench]]` plumbing pattern). **Recommended for the project + keystroke benches** — they
  drive the `gdscript-ide` public API, which is exactly what `analysis.rs` already does.
- **(B) A dedicated `benches/` workspace crate** for cross-crate benches (parse throughput touches
  `gdscript-syntax`; memory profiling wants its own bin). Add it to `[workspace.members]` and
  `default-members` carefully (benches must stay **native-only** — `criterion` pulls `getrandom`/
  `rayon`, which the wasm portability guard forbids; benches are dev/native, so they never enter the
  wasm graph, but keep them out of `gdscript-wasm`'s dependency closure).

**Recommendation:** put project/keystroke/completion benches in `gdscript-ide/benches/project.rs`
(register `[[bench]] name = "project" harness = false`), and parse-throughput in
`gdscript-syntax/benches/parse.rs`. Both reuse the existing single `[[bench]]` registration idiom.

**Bench sketches** (matching the existing `analysis.rs` idioms — `criterion_group!`/`criterion_main!`,
`black_box`, a warmed engine model):

```rust
// crates/gdscript-ide/benches/project.rs  (sketch)
use criterion::{Criterion, criterion_group, criterion_main, BenchmarkId};
use std::hint::black_box;
use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{AnalysisHost, Change};

/// Load every .gd + .tscn under a perf tier into ONE host (mirrors examples/corpus.rs --project),
/// returning the host, the FileId of the manifest `hot_file`, and the resolved edit/complete spans.
fn load_tier(tier: &Manifest) -> Loaded { /* read_dir, set_file_path(res://…), set_project_config */ }

fn cold_full_project(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_full_project");
    for tier in ["small", "medium", "large"] {
        let m = Manifest::load(tier);
        group.bench_with_input(BenchmarkId::from_parameter(tier), &m, |b, m| {
            // Cold = a fresh host each iteration (criterion's per-iter setup), so nothing is memoized.
            b.iter_batched(
                || load_inputs(m),                      // build the Change set (untimed)
                |inputs| {
                    let mut host = AnalysisHost::new();
                    host.apply_change(inputs);
                    let a = host.analysis();
                    for f in m.file_ids() { black_box(a.diagnostics(black_box(f)).unwrap()); }
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }
}

fn warm_keystroke(c: &mut Criterion) {
    let mut group = c.benchmark_group("warm_keystroke");
    for tier in ["small", "medium", "large"] {
        let m = Manifest::load(tier);
        let mut host = AnalysisHost::new();
        host.apply_change(load_inputs(&m));
        let _ = host.analysis().diagnostics(m.hot_file_id()).unwrap();  // warm the cache
        group.bench_with_input(BenchmarkId::from_parameter(tier), &m, |b, m| {
            let mut toggle = false;
            b.iter(|| {
                // A body-only edit (LOW durability) — the keystroke. Two variants alternate so the
                // text actually changes each iter (salsa no-ops an identical set's downstream).
                let mut ch = Change::new();
                ch.change_file(m.hot_file_id(), m.body_edit_variant(toggle)); toggle = !toggle;
                host.apply_change(ch);                     // bumps the LOW revision
                black_box(host.analysis().diagnostics(black_box(m.hot_file_id())).unwrap());
            });
        });
        // The §4.2 ASSERTION lives in a #[test], not the bench: warm_keystroke wall-clock must be
        // FLAT across small/medium/large (slope ≈ 0). Criterion reports per-tier; the test compares.
    }
}
```

The **bounded-invalidation** metric is **not** a wall-clock bench — it is a **recount assertion**,
lifting the `queries.rs` `WITNESS_RUNS` idiom into a reusable harness. Two implementation options:

- **(A) salsa event log.** salsa 0.27 exposes a `salsa::Database` event hook
  (`salsa::Storage::new(Some(Box::new(|ev| …)))` / the `salsa_event` callback). Build a
  `LoggingDatabase` test wrapper that records `EventKind::WillExecute { database_key }` and lets a
  test assert *exactly which queries re-ran* after an edit. This is rust-analyzer's
  `assert_unchanged`/`assert_reparse`-style recount — the most precise, and reusable across signature
  edits, `class_name` edits, and project-config edits.
- **(B) counting witness query** (the existing idiom, generalized). Keep the `AtomicU32` +
  `#[salsa::tracked]` witness, but parameterize it so a test can target any tier's hot file.

**Recommendation: (A)** — a `LoggingDatabase` recorder is the canonical rust-analyzer pattern and
turns "bounded invalidation" into a *count of re-executed queries*, which is what §4.2 and the parent
plan's exit criterion actually want. Land it in `gdscript-db` behind `#[cfg(any(test, feature =
"bench-harness"))]` so both `#[test]` and the criterion bench can use it.

```rust
// crates/gdscript-db/src/bench_harness.rs  (sketch — behind a feature)
pub struct RecountDb { inner: RootDatabase, log: Arc<Mutex<Vec<String>>> }
impl RecountDb {
    pub fn took_executions(&self) -> usize { self.log.lock().unwrap().len() }
    pub fn reset(&self) { self.log.lock().unwrap().clear(); }
}
// salsa_event hook pushes the demangled query name on EventKind::WillExecute.
// Test: edit a body → assert reg/infer queries for OTHER files did NOT re-execute.
//       edit a signature → assert ONLY this file's dependents re-execute, not the world.
//       edit a class_name → assert global_registry recomputes + ONLY referencing files re-infer.
```

### 3.3 The metrics table (verified against the code's real targets)

| Metric | Target | How measured | Code anchor |
|---|---|---|---|
| **Cold full-project analyze** (large) | budget from first measurement, **±10 % guarded** | `cold_full_project` bench, fresh host per iter | `examples/corpus.rs` loop, timed |
| **Warm keystroke** (edit one body, re-`diagnostics`) | **< 10 ms**, **flat** as tier grows | `warm_keystroke` bench + a slope `#[test]` | `apply_change` LOW edit (`lib.rs:111`) |
| **Bounded invalidation** (signature / `class_name` edit) | re-executes **only dependents**, not the world | `RecountDb` event log, `#[test]` | `item_tree` backdate firewall (`queries.rs:471`) |
| **Member completion** (`recv.`) warm | **< 5 ms** | `member_completion` bench (carry from `analysis.rs`) | `Analysis::completions` |
| **Parse throughput** | **MB/s** baseline, ±10 % guarded | `gdscript-syntax/benches/parse.rs` over the fixture | `gdscript_syntax::parse` (`db lib.rs:173`) |
| **Engine-model decode** (cold start tax) | one-time, **once per process** | dhat + a startup `#[test]` asserting `OnceLock` hit once | `gdscript-api::bundled()` (`lib.rs:161`) |

**Note on the warm/keystroke target:** the existing `analysis.rs` targets (cold < 50 ms, warm < 5 ms)
were **single-file Phase-2** numbers *without a salsa cache*. With salsa (Phase 3+), the warm keystroke
on a body edit should be **dominated by re-`infer` ing one body + re-running `diagnostics` for one
file**, which the firewall keeps independent of project size — hence the **< 10 ms, flat** target. The
flatness is the real promise; the absolute 10 ms is the ceiling.

### 3.4 Memory profiling (dhat)

`dhat` is **not yet a dependency** (`grep dhat Cargo.lock` → empty). Add `dhat = "0.3"` as a workspace
dev-dep and a feature-gated heap profiler:

```rust
// crates/gdscript-ide/examples/memprofile.rs  (run: cargo run --release --example memprofile --features dhat -- fixtures/perf/large)
#[cfg(feature = "dhat")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
fn main() {
    #[cfg(feature = "dhat")] let _profiler = dhat::Profiler::new_heap();
    // load the large tier into one host (corpus.rs --project path), analyze every file,
    // then hold the host live while dhat snapshots peak + resident.
}
```

**What to assert / document** (from §4.3 of the parent + the code):

- The **engine model is loaded once**, not per file. ✔ already true (`OnceLock` native; the leaked
  `&'static` on wasm). dhat confirms a single 3.47 MB-ish decode, not N copies. **Add a `#[test]`**
  that drives many files through one host and asserts `bundled()` returns the same pointer each call
  (cheap structural proof, complementing the dhat snapshot).
- **`Ty` stays small + `Copy`** via interning (`ClassId`/`BuiltinId` are `u32` newtypes;
  `gdscript-api` uses them as map keys). Document the `size_of::<Ty>()` in the memory page; ideally a
  `#[test]` pins it (`assert_eq!(size_of::<Ty>(), N)`) so a future field bloat is caught.
- **The eager `from_bytes` decode** (owned `ApiData` + rebuilt `FxHashMap` indices, `lib.rs:84-140`)
  is the dominant fixed cost. Record it. If it dwarfs the per-file analysis state, that is the lever
  for the deferred zero-copy-archived rewrite (§1 tail).
- **Resident analysis state caps** under LRU eviction (§3.5) — dhat before/after eviction on the large
  tier proves the bound.

`heaptrack` is a fine native alternative but Linux-centric; **dhat is cross-platform and Rust-native**
(works in the same `cargo run`), so prefer it for the committed profiler; mention heaptrack as a
deeper ad-hoc option in `fixtures/perf/README.md`.

### 3.5 Salsa LRU eviction for cold files

**Problem:** a 1000+-file project memoizes `parse`/`item_tree`/`infer` for every file. Hot editing
touches a handful; the rest are cold but resident. rust-analyzer bounds this with **LRU eviction** on
derived queries.

**Plan:** use salsa 0.27's LRU support. In new-salsa, eviction is configured **per tracked query** —
e.g. `#[salsa::tracked(lru = N)]` caps the memoized entries for that query, evicting least-recently-
used. Apply LRU to the **expensive, file-granular, cold-evictable** queries — primarily **`parse`**
(`db lib.rs:172`, the lossless CST is the biggest per-file allocation) and the per-file `infer`/
`analyze_file` in `gdscript-hir`. **Do NOT** LRU the project-global queries (`global_registry`,
`source_root`, autoload index) — they are MEDIUM-durability and cheap to keep; evicting them would
thrash the firewall.

```rust
// crates/gdscript-db/src/lib.rs — cap parse's memo table (sketch; confirm exact salsa 0.27 syntax)
#[salsa::tracked(lru = 128)]            // keep the 128 most-recently-parsed files hot
pub fn parse(db: &dyn Db, file: FileText) -> Parse { gdscript_syntax::parse(file.text(db)) }
```

**Caveats to verify before wiring:**
- Confirm the **exact LRU attribute syntax** in the pinned salsa version (`grep salsa Cargo.toml`
  shows the version; check the `salsa` crate docs — the macro arg may be `lru = N` or a builder).
  ⚠ If the pinned salsa version does **not** expose LRU on tracked fns, the fallback is **manual GC**:
  call `db` cache-clearing on cold `FileId`s the loader marks closed (the `close`/`remove_file` path
  in `Session` already drops side-table entries; extend it to drop derived memos).
- LRU must **not** break the firewall recount tests — an evicted-then-recomputed query *will*
  re-execute, which is correct but must not be mistaken for an invalidation regression. The
  `RecountDb` harness (§3.2) must keep LRU off (or N large) so recount assertions measure
  *invalidation*, not *eviction*.
- LRU is **native-relevant**; on wasm the project is small (a playground snippet), so eviction is
  inert there — keep the cap generous so it never bites single-file/playground use.

### 3.6 The wasm bundle-size budget

| Artifact | v1 budget | Current | Strategy + what's missing |
|---|---|---|---|
| wasm code module (brotli) | **≤ ~1.5 MB** | **≈777 KB raw** (`playground/pkg/gdscript_bg.wasm`), well under even pre-brotli | `wasm-opt -Oz` **✔ already configured** (`bindings/wasm/Cargo.toml` `[package.metadata.wasm-pack.profile.release]`). **Missing:** brotli the published `.wasm` + a size gate. Optional: also strip via a post-build step (wasm-pack `--release` doesn't apply the `wasm-release` profile's `strip`). |
| Engine-API data asset (brotli) | **≤ ~few-hundred KB** wire | **3.47 MB raw rkyv**, **un-pruned, un-compressed** (`engine_api.bin`) | ⚠ **The biggest gap.** Today it is `include_bytes!`'d on native and must be *fetched* on wasm — but **no pruned/brotli/content-hashed asset pipeline exists**. Build it (§ below). |

**The engine-asset pipeline (new — `xtask wasm-dist` + `release-wasm.yml`):**

1. **Prune** the rkyv blob for the browser (drop doc strings / rarely-needed metadata the playground
   doesn't surface) → a smaller `ApiData` variant, or keep full and rely on brotli. Measure both.
2. **Brotli-compress** the (pruned) blob → `engine_api.<hash>.bin.br`. brotli is **not yet a dep** —
   add `brotli` to the xtask deps (xtask is native-only, so no wasm-portability concern).
3. **Content-hash** the filename and emit it next to the wasm in the published `pkg/` (or a CDN
   path), so the page `fetch`es a cache-bustable asset, decompresses (brotli WASM or the browser's
   `DecompressionStream` where available — note `DecompressionStream` supports gzip/deflate, **not**
   brotli, so ship a tiny brotli-wasm decompressor **or** serve with `Content-Encoding: br` and let
   the CDN/browser inflate transparently — **the transparent-CDN path is simplest; document it**).
4. The page calls `Analyzer.loadEngineApi(bytes)` (already wired — `bindings/wasm/src/lib.rs:60`) with
   the decompressed `ArrayBuffer`. `from_bytes` already aligns + validates (`api/src/lib.rs:134`).

**Decision:** ⚠ the parent plan says *"the existing wasm-opt -Oz + brotli set_engine_api path."* The
`-Oz` and `set_engine_api`/`loadEngineApi` **functions** exist; the **brotli + content-hashed asset
build/serve path does not** — it is **net-new in this workstream**. Build it under
`xtask wasm-dist` and exercise it in `release-wasm.yml` (and a CI smoke test that fetches + loads it
headless — the parent Testing §6 "smoke test loads the wasm + pruned API asset").

### 3.7 CI regression guards

**Bench guard (new job).** criterion supports baselines (`--save-baseline` / `--baseline`). For a CI
gate that *fails* on regression, the cleanest 2026 option is **`codspeed`** (`cargo codspeed` + the
CodSpeed GitHub Action, which runs criterion-compatible benches in a stabilized runner and comments
regressions on the PR) **or** **Bencher** (`bencher run … --err`). Pure-criterion `--baseline`
comparison is noisy on shared CI runners — call this out.

```yaml
# .github/workflows/bench.yml  (sketch)
name: bench
on: { pull_request: {}, push: { branches: [master] } }
jobs:
  bench:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      # Option A — CodSpeed (recommended: stabilized timing, PR comments, >10% gate):
      - uses: CodSpeedHQ/action@v3
        with: { run: "cargo codspeed run -p gdscript-ide -p gdscript-syntax", token: ${{ secrets.CODSPEED_TOKEN }} }
      # Option B — Bencher fallback (self-thresholded):
      #   bencher run --project gdscript-analyzer --err --adapter rust_criterion "cargo bench"
```

The **flat-as-project-grows** and **bounded-invalidation** assertions are **`#[test]`s** (slope check;
`RecountDb` count), which run in the normal `test` job — they fail deterministically without depending
on noisy wall-clock, so they are the **hard** gates; the wall-clock benches (cold/keystroke/parse/
completion) are the **±10 % advisory** gates via CodSpeed/Bencher.

**Wasm-size guard (new step, in `ci.yml` `wasm` job or `release-wasm.yml`).**

```yaml
- name: wasm size budget
  run: |
    wasm-pack build --target web --release --out-dir pkg bindings/wasm
    WASM=pkg/gdscript_bg.wasm
    RAW=$(stat -c%s "$WASM")
    BR=$(brotli -c "$WASM" | wc -c)
    echo "raw=$RAW brotli=$BR"
    # Hard ceilings (bytes): wasm brotli ≤ 1.5MB, raw ≤ 2.5MB. Fail over budget.
    test "$BR" -le 1572864 || { echo "::error::wasm brotli $BR over 1.5MB budget"; exit 1; }
```

Plus a **byte ceiling on the brotli'd engine asset** in the same job. The existing `wasm` portability
guard (`ci.yml:87`) and `getrandom`-out assertion **stay** — they are the correctness floor the size
work must not break.

---

## 4. Step-by-step implementation plan

**M0 — the fixture + the cold bench (unblocks everything).**
1. Pick + vendor the real game under `fixtures/perf/{small,medium,large}/` with `README.md`
   provenance/license (mirror `vendor/godot/4.5-stable/SOURCE.txt`) and `manifest.toml` with the
   marker-located edit/complete spans.
2. Add `crates/gdscript-ide/benches/project.rs` (`[[bench]] name = "project" harness = false`), seeded
   from `examples/corpus.rs --project`'s load loop. Land `cold_full_project` over all three tiers.
3. *Exit:* `cargo bench -p gdscript-ide --bench project` prints per-tier cold numbers; commit them as
   the baseline.

**M1 — warm keystroke + bounded invalidation (the headline incremental metrics).**
4. Build the `RecountDb` salsa-event harness in `gdscript-db` (behind `#[cfg(any(test, feature =
   "bench-harness"))]`), lifting the `WITNESS_RUNS`/`REGISTRY_OBSERVED` idiom from `queries.rs` into a
   reusable `WillExecute` recorder.
5. Add `warm_keystroke` bench (body-edit → re-`diagnostics`) + the **slope `#[test]`** asserting
   flatness across tiers (< 10 ms each, slope ≈ 0).
6. Add **recount `#[test]`s**: body edit (no global re-exec), **signature edit** (only dependents),
   **`class_name` edit** (registry recomputes + only referencing files re-infer). These reuse the
   firewall the code already guarantees — they extend the existing two firewall tests to the
   fixture scale.
7. *Exit:* the incremental promise is a **deterministic test**, not a vibe.

**M2 — completion + parse throughput.**
8. Port `member_completion` from `analysis.rs` to the fixture (use the manifest `completion_site`).
9. Add `crates/gdscript-syntax/benches/parse.rs` reporting **MB/s** over the fixture corpus (criterion
   `Throughput::Bytes`).
10. *Exit:* all five §4.2 wall-clock metrics produce numbers.

**M3 — memory + LRU.**
11. Add `dhat` dev-dep + `examples/memprofile.rs` (`--features dhat`); document peak/resident on the
    large tier in a `docs/` perf page; add the `size_of::<Ty>()` pin test + the "engine loaded once"
    pointer test.
12. Wire LRU on `parse` (+ `infer`/`analyze_file`) — **verify the salsa 0.27 LRU syntax first**;
    fall back to manual cold-`FileId` GC via the `close`/`remove_file` path if LRU isn't exposed.
    Re-run dhat to prove the bound. Ensure the `RecountDb` harness disables LRU.
13. *Exit:* large-tier resident set is bounded + documented.

**M4 — the wasm bundle pipeline.**
14. Add `xtask wasm-dist`: `wasm-pack build --target web --release` → (optional strip) → record
    `.wasm` raw + brotli size; **prune + brotli + content-hash** `engine_api.bin` → emit the asset +
    a manifest (`{ wasm, engine_asset, hashes }`). Add `brotli` to xtask deps.
15. Wire the playground page to `fetch` the content-hashed brotli engine asset and `loadEngineApi`
    (the function already exists — `bindings/wasm/src/lib.rs:60`).
16. *Exit:* the playground loads engine-class completion from a fetched, brotli'd, few-hundred-KB
    asset; the `.wasm` is measured.

**M5 — the CI guards.**
17. Add the `bench` workflow (CodSpeed or Bencher, **> 10 %** gate) running `gdscript-ide` +
    `gdscript-syntax` benches.
18. Add the **wasm-size step** (brotli ceiling) to `ci.yml`/`release-wasm.yml`; add the **engine-asset
    size ceiling**. Add a headless smoke test that fetches + loads the asset and analyzes a snippet.
19. Mirror the new gates in `xtask ci` (`tasks.rs::ci`) so contributors get them locally.
20. *Exit:* a PR that regresses any tracked metric > 10 % or blows the byte ceiling **fails**.

---

## 5. Test plan

1. **Bench-as-test (the hard gates).**
   - **Flatness:** a `#[test]` runs `warm_keystroke` timing for small/medium/large (few iters,
     `Instant`-based, native-only) and asserts the per-edit time does **not** grow with tier (ratio
     large/small < a tolerance, e.g. 1.5×) and each is **< 10 ms**.
   - **Bounded invalidation (recount):** `RecountDb` `#[test]`s — body edit re-executes **0**
     cross-file queries; signature edit re-executes **only this file's dependents**; `class_name`
     edit re-executes `global_registry` + **only referencing files**. These extend
     `body_edit_does_not_invalidate_signature_queries` / `…_the_global_registry`
     (`queries.rs:471/540`) to fixture scale.
   - **Engine-loaded-once:** drive N files through one host; assert `bundled()`/the `&'static`
     pointer is identical across calls and dhat shows a single decode.
   - **`Ty` size pin:** `assert_eq!(size_of::<Ty>(), N)` so field bloat is caught.
2. **Differential / correctness on the fixture (no behavior change from perf work).** Run
   `examples/corpus.rs --project fixtures/perf/large` before/after LRU + before/after the wasm-asset
   change and assert **identical diagnostic counts + zero panics** — LRU eviction and pruning must be
   **observationally transparent** (an evicted query recomputes to the *same* value). This reuses the
   existing corpus runner as a golden oracle.
3. **Memory regression (advisory).** `memprofile` peak/resident on the large tier recorded; a soft CI
   note if it grows > 10 % (not a hard fail — allocation counts are platform-sensitive).
4. **Wasm-size golden.** The CI byte ceilings (§3.7) — `.wasm` brotli ≤ 1.5 MB, engine asset brotli ≤
   its ceiling. A headless **smoke test**: instantiate the wasm, `loadEngineApi(fetched_asset)`,
   `openDocument` a snippet using `Button`, assert engine-class completion is non-empty (proves the
   fetched-asset path end-to-end).
5. **Property test (LRU soundness).** Random sequences of `open`/`change`/`close`/query over the
   medium tier with LRU on vs LRU off must yield **identical query results** (eviction never changes
   the answer, only the cache footprint). proptest is already a dep (`Cargo.toml:125`).
6. **Bench-baseline regression (the CodSpeed/Bencher gate).** > 10 % regression of cold/keystroke/
   parse/completion fails the PR. Commit the first run as the baseline.

---

## 6. Risks + mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **CI bench noise** — shared GitHub runners give ±10–30 % wall-clock variance, so a naive criterion `--baseline` gate flaps. | **High** | Make the **hard** gates the deterministic `#[test]`s (flatness ratio, recount counts, byte ceilings); use **CodSpeed** (stabilized instruction-count runner) or **Bencher** with a statistical threshold for the *advisory* wall-clock gates, not raw criterion on a noisy runner. |
| **Fixture license / size** — vendoring a multi-thousand-file game bloats the repo + risks a license mismatch. | Med | MIT/CC0/BSD only, provenance recorded; consider git-LFS or a pinned submodule for the **large** tier if it's heavy; the **small/medium** tiers (the PR-signal tiers) stay in-tree and lean. |
| **salsa LRU API uncertainty** — the pinned salsa version may not expose `lru = N` on tracked fns, or may have changed it. | Med | **Verify the exact attribute against the pinned `salsa` version before coding** (M3 step 12); fall back to **manual cold-`FileId` GC** through the existing `close`/`remove_file` path. Don't block the rest of the workstream on it. |
| **LRU eviction confused with invalidation** in recount tests — an evicted query recomputes, inflating the count. | Med | Keep LRU **off / N-large** in the `RecountDb` harness; the property test (§5.5) separately proves eviction is value-transparent. |
| **Brotli-in-browser** — `DecompressionStream` doesn't support brotli; shipping a brotli-wasm decoder re-bloats the bundle. | Med | Prefer **transparent `Content-Encoding: br`** at the CDN/host so the browser inflates for free; document the host requirement. Only ship a brotli-wasm decoder if a no-CDN static host is a hard requirement. |
| **Eager engine decode dominates cold start** — `from_bytes` fully deserializes 3.47 MB + rebuilds indices. | Med | Measure it (M3); if it dwarfs analysis state, schedule the **zero-copy archived `rkyv::access`** rewrite (deferred §1) — but only if the budget actually fails; the `OnceLock` already amortizes it to once-per-process. |
| **`getrandom`/`rayon` leaking into wasm via benches.** | Low | Benches are native dev-targets, never in `gdscript-wasm`'s closure; the existing `getrandom`-out CI assertion (`ci.yml:107`) stays and would catch a leak. |
| **Doc drift on durability** — docs say LOW/MEDIUM/HIGH; code has LOW/MEDIUM + an out-of-salsa engine. | Low | **Correct the perf docs to the two-tier reality** (§2.1); if a future query needs salsa to track the engine, promote it to a `Durability::HIGH` input then — not before. |

---

## 7. Dependencies on other workstreams

- **Workstream 1 (full warning set) & 2 (CFG narrowing)** *add* per-body work (`flow(body)`,
  more checks). The warm-keystroke + bounded-invalidation benches must run **after** those land (or be
  re-baselined when they do), since they change the per-edit compute. The **gating layer is
  deliberately cheap + re-run per snapshot** (parent §1.1) — a bench should confirm editing a
  **warning setting** does **not** invalidate `infer`/`flow` (a recount test, parent Testing §2). The
  new `flow(body)` query is a **prime LRU candidate** (§3.5) — coordinate the LRU caps with WS2.
- **Workstream 3 (formatter)** adds a CST→`Doc` pass; if it ships an LSP `formatting`/CLI `format`
  path, add a **format-throughput** bench to the same harness (idempotence is WS3's own test; *speed*
  is this workstream's). Low coupling.
- **Workstream 5 (docs)** consumes this workstream's **perf page** (metrics table + memory numbers)
  and the **playground** depends on the §3.6 wasm/engine-asset pipeline being live (the playground is
  "live docs"). The wasm-size guard protects the playground UX gate WS5 relies on.
- **Workstream 6 (API stabilization)** — the bench harness drives the **frozen `gdscript-ide` public
  API** (`AnalysisHost`/`Analysis`/`Change`), so benches double as a **stability exerciser** before
  the 1.0 freeze; keep the `RecountDb`/`bench-harness` feature **internal** (not part of the
  semver'd surface).
- **Phase-3 invariants** are the foundation: this workstream **measures and guards** the
  body-edit firewall + durability split that Phase 3 built (`gdscript-db`, `queries.rs`). It adds
  **no new architectural layer** — it adds fixtures, benches, a memory profiler, LRU caps, a wasm
  asset pipeline, and CI gates.

---

## 8. Corrections to the canonical plan (honesty pass)

| Parent §4 claim | Reality in the code | Action |
|---|---|---|
| "salsa durability … LOW/MEDIUM/HIGH" | Only **LOW** (file text) + **MEDIUM** (res_path, source_root, project_config, engine_gen) are used; **no `Durability::HIGH`**. The engine model is an out-of-salsa leaked `&'static` + `OnceLock`. | Document two-tier; treat the engine as durable-by-being-outside-salsa. |
| "the existing **wasm-opt -Oz + brotli** set_engine_api path" | `wasm-opt -Oz` **is** configured (`bindings/wasm/Cargo.toml`). **brotli + content-hashed engine asset = net-new**; `set_engine_api`/`loadEngineApi` exist but consume a *raw* `include_bytes!`/un-pipelined blob today. | Build the brotli + content-hash + fetch pipeline (M4). |
| "the existing benches" | **One** Phase-2 single-file bench (`gdscript-ide/benches/analysis.rs`); repo-root `benches/` is an empty `.gitkeep`. | Add project/keystroke/parse benches. |
| "salsa LRU eviction for cold files" | **Not implemented**; verify the pinned-salsa LRU API exists before relying on it. | M3 step 12 + manual-GC fallback. |
| "a test that counts query re-executions" | **Exists** — `WITNESS_RUNS`/`REGISTRY_OBSERVED`/`RES_REGISTRY_OBSERVED`/`AUTOLOAD_OBSERVED` AtomicU32 + tracked-witness idiom in `crates/gdscript-hir/src/queries.rs`. | **Reuse it** — lift into the `RecountDb` harness; don't reinvent. |
| `fixtures/perf/` vendored fixture | **Does not exist** (`fixtures/` = `ide/.gitkeep` + `parser/.gitkeep`). | M0 — vendor it first. |
| memory profiling (dhat) | `dhat` **not a dependency**. | Add it (M3). |
| CI bench + wasm-size guards | **Neither exists** in `ci.yml`/`release-wasm.yml`/`xtask`. | M5 — add both. |
