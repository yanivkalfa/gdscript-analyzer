# Phase 5 · Workstream 3 — `gdscript-cli` Playbook

> Research-backed build plan for the `gdscript` CLI binary (`gdscript-cli` crate) over the
> `gdscript-ide` `AnalysisHost`/`Analysis` API. 2026-current, adversarially verified (24/24 confirmed;
> 1 refuted — see §SARIF). Mirrors the LSP playbook format.
>
> **Parents:** [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) §Workstream 3,
> [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §7 (the CLI is the only layer that touches the filesystem).

---

## 0. Thesis

The CLI is a **thin filesystem + presentation shell** over `gdscript-ide`. Its only real engineering
is (a) the **0-based UTF-8 byte-offset → 1-based line:col** conversion every CI consumer needs (the
#1 correctness risk — own it once, in a `LineIndex`), and (b) the **load-all-then-fan-out** batch
model that keeps cross-file resolution while parallelising the reads.

---

## 1. Command tree & flags (clap v4 derive)

`gdscript <command> [paths…]` — paths default to `.`; `-` = stdin (single-file).

| Command | Does | Query |
|---|---|---|
| `check` | parse + type diagnostics across the project (CI workhorse). | `diagnostics(file)` ∀ file |
| `lint` | diagnostics filtered to the warning/lint subset. | `diagnostics` + severity filter |
| `format [--check\|--write]` | format-in-place / check. **Phase 5 = a passthrough** (identity) with a clear note; the real formatter is Phase 6. | `format` (Phase 6) |
| `symbols` | dump document symbols as JSON (for tooling / AI agents). | `document_symbols(file)` |

Global flags (Ruff/Biome precedent): `--format <human\|json\|github\|sarif\|rdjson>` (default `human`),
`--output-file <path>`, `--config <file-or-KEY=VALUE>`, `--no-config` (a.k.a. `--isolated`),
`--error-on-warning`, `--quiet`, `--no-color` (also honor `NO_COLOR`/`CLICOLOR` via anstream).

---

## 2. Output formats (exact, verified)

A single internal `Finding` (file path + the POD `Diagnostic`) feeds every emitter. **Collect all →
stable-sort by `(path, line, col)` → emit** (deterministic under rayon).

- **human** (default): `annotate-snippets` v0.12.16 (rustc-style: severity + `path:line:col` header +
  numbered source + caret over the byte span). Color via `anstream::AutoStream` (strips ANSI for
  non-ttys; respects `NO_COLOR`/`CLICOLOR`). 1-based line:col.
- **json**: a stable array of the serde `Diagnostic` POD **plus** 1-based `line`/`col` (and the byte
  range) per item, so programmatic consumers don't re-derive positions.
- **github** (CI annotations): **exact** workflow-command syntax
  `::{error|warning|notice} file={f},line={l},endLine={el},col={c},endColumn={ec},title={code}::{msg}`,
  1-based line/col. **Two escapers** (from `actions/toolkit` `command.ts`):
  - message → `escapeData`: `%`→`%25`, `\r`→`%0D`, `\n`→`%0A`.
  - property values (file/line/col/title) → `escapeProperty`: the same three **plus** `:`→`%3A`,
    `,`→`%2C`.
- **sarif**: SARIF **2.1.0** (the only version GitHub code-scanning accepts), 1-based lines+columns,
  **exclusive** `endColumn`/`endLine` (OASIS). ⚠️ The "minimal required-property set" claim was
  *refuted* — emit a standard `runs[].tool.driver{name,rules[]}` + `results[].{message,locations[]}`
  with `physicalLocation.artifactLocation.uri` + `region` and **validate against GitHub's actual
  ingester** before treating it as done. GitHub's `endLine` wording reads inclusive vs OASIS
  exclusive — a real ambiguity to test.
- **rdjson** (reviewdog): 1-based line + **UTF-8 byte** column (uniquely matches our byte core — no
  code-point/UTF-16 projection). Lowest-friction CI format for us.

**Column unit per format (the subtle part):** rdjson = UTF-8 **byte** column; github/sarif are
consumed as **character** columns; annotate-snippets renders from the **byte span** itself. So the
`LineIndex` exposes *both* a byte-column and a char-column projection; each emitter picks.

---

## 3. The `LineIndex` (the #1 correctness component)

Per-file newline-offset table built once. Given a `u32` byte offset → `(line, col)` **1-based**, with:
- `char_col` (Unicode scalar count from line start) — for human/github/sarif.
- `byte_col` (UTF-8 byte count from line start) — for rdjson.

Mandatory tests: multi-byte (`된장`), astral (`🎮`), CRLF, BOM, offset at EOF, empty file — the same
corpus that bit the LSP. (This is the CLI's analogue of the LSP's `LineIndex`; it outputs **1-based**
where the LSP output 0-based.)

---

## 4. Project model, config discovery & traversal

- **Traversal:** the `ignore` crate (ripgrep's — honors `.gitignore`/`.gdignore`, parallel walk) over
  the target paths, collecting `*.gd`. A path that is a file is taken directly; `-` reads stdin.
- **Project root:** walk up from each target to the nearest `project.godot` (the Godot marker → the
  Phase-3 project model: feeds `set_project_config` for autoloads + version, and anchors `res://`).
- **`res://` paths:** each `.gd` file's path **relative to its project root** → `res://<rel>`, fed via
  `Change::set_file_path` so cross-file resolution (`class_name`, `preload`, autoloads) works.
- **Analyzer config:** optional `gdscript-analyzer.toml` (or a `[tool.gdscript]` table) — **nearest-wins**
  (Ruff model: the closest config per file; **no** merge-up). `--config <file>` applies to all files
  (relative paths vs CWD) or inline `KEY=VALUE` overrides; `--no-config`/`--isolated` skips discovery.
  *Phase 5 ships discovery + plumbing; the actual gating/format options the config carries are minimal
  until Phase 6's warning taxonomy + formatter exist.*

---

## 5. The batch engine — load-all → fan-out (the leverage point)

The verified-best model (rust-analyzer / Ruff precedent), matching our single-writer/many-reader core:

1. **Load (serial):** one `AnalysisHost`. `apply_change` the `project.godot` text + **every** `.gd`
   file's text + its `res://` path. One `sync_source_root`. (Cross-file resolution now works.)
2. **Snapshot:** `let analysis = host.analysis();` (cheap, `Clone + Send`).
3. **Fan out (parallel):** `rayon` over the files; each task clones `analysis` and runs
   `diagnostics(file)`. No concurrent writes during the read phase ⇒ nothing cancels.
4. **Collect → sort `(path,line,col)` → emit.**

`symbols` is the same shape with `document_symbols`. (Benchmark the serial load vs. parallel reads on
a big project; shard only if the load dominates — a documented follow-up, not a v1 need.)

---

## 6. Exit codes & robustness

| Code | Meaning |
|---|---|
| 0 | Clean (no diagnostics at/above the fail threshold; `format --check` found nothing). |
| 1 | Diagnostics found (check/lint) or files would reformat (`format --check`). |
| 2 | Usage/config error (bad args, unreadable config). |
| `>2` (use **101**) | Internal error — a **panic hook** catches it, prints a bug-report line, exits 101 (matches Rust's default panic code). |

`--error-on-warning` promotes warnings into the `1` bucket (default: warnings are non-fatal; errors
always fail). UX: `indicatif` progress only when `stderr.is_terminal()`; never on a pipe/CI.

---

## 7. Distribution (brief — full GA is Workstream 5)

A single static `gdscript` binary, **no Godot / no network / no dynamic libs**. cargo-dist
per-platform archives + an install script (wired in GA). A `.pre-commit-hooks.yaml` entry so the same
binary serves pre-commit + CI (gdtoolkit/Ruff precedent).

---

## 8. Risks (rated)

| Risk | Sev | Mitigation |
|---|---|---|
| **byte→1-based line:col + per-format column unit** | **Critical** | the §3 `LineIndex` with byte+char projections + the multi-byte/astral/CRLF golden corpus; one converter, every emitter consumes it. |
| Non-deterministic output under rayon | High | final stable sort on `(path,line,col)`. |
| Color leaking into pipes/CI | Med | `anstream::AutoStream` everywhere; `--no-color`/`NO_COLOR`. |
| Exit-code mismatch breaks CI gates | Med | the §6 table + tests asserting each code; panic hook → 101. |
| github escaping (`,`/`:`/newline) | Med | the two exact escapers (§2); golden tests with commas/colons/newlines in messages. |
| SARIF shape GitHub won't ingest | Med | standard 2.1.0; validate vs the real ingester (open question). |
| Huge-output buffering | Low | stream human/github per file; json/sarif are documents (buffer, size the cost). |

**Biggest correctness risk:** the position conversion (§3). **Biggest leverage:** the load-all →
fan-out batch model (§5) — cross-file-correct *and* embarrassingly parallel.

---

## 9. Milestones (each green through `xtask ci`)

- **M0 — spine:** clap tree + `LineIndex` (+ golden tests) + the load→fan-out engine + `check` with
  **human** output + exit codes + the panic hook. *Exit:* `gdscript check <dir>` prints rustc-style
  diagnostics at correct 1-based positions and exits 0/1.
- **M1 — formats:** json, github (+ escapers), sarif, rdjson + `--output-file`. Golden tests per
  format (incl. the escaping edge cases).
- **M2 — commands:** `lint` (severity filter + `--error-on-warning`), `symbols` (JSON), `format`
  (passthrough w/ `--check`/`--write` plumbing + a clear "Phase-6" note), config/project discovery
  (`project.godot` + `res://` + nearest-wins analyzer config, `--config`/`--no-config`).
- Per-milestone **adversarial bug-hunt** (find→verify→fix), like every prior milestone.

## Sources (verified, 2026)
Ruff (config discovery, `--output-format`, exit codes), Biome (`--reporter`, `--error-on-warnings`),
GitHub Actions workflow-commands + `actions/toolkit` `command.ts` (the two escapers), OASIS SARIF
2.1.0, reviewdog RDF (1-based byte column), `annotate-snippets` 0.12.16, `anstream::AutoStream`,
`ignore`, cargo-dist, pre-commit. **Refuted:** a specific "minimal SARIF required-property set" (0-3)
— validate emitted SARIF empirically.
