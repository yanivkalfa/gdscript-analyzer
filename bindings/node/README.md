# @gdscript-analyzer/core

**Native Node.js bindings for [gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer) — a fast, embeddable GDScript (Godot 4.x) static-analysis library. "Roslyn for Godot."**

[![npm](https://img.shields.io/npm/v/@gdscript-analyzer/core?logo=npm)](https://www.npmjs.com/package/@gdscript-analyzer/core)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/yanivkalfa/gdscript-analyzer#license)

Analyze GDScript **headlessly** — no running Godot editor — from Node: diagnostics,
type-aware hover, completion, go-to-definition, find-references, rename, document
& workspace symbols, signature help, folding ranges, inlay hints, and code
actions. This is the **native** binding (a [napi-rs](https://napi.rs) addon, no
WASM overhead); for the browser use [`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm).

```sh
npm i @gdscript-analyzer/core
```

Prebuilt binaries ship for **macOS** (x64 / arm64), **Windows** (x64), and
**Linux** (x64 / arm64-gnu); npm installs only the one for your platform.

---

## Quick start

```js
import { AnalysisHandle } from "@gdscript-analyzer/core";

const az = new AnalysisHandle();
const uri = "inmemory://player.gd";

az.openDocument(uri, `extends Node

func _ready() -> void:
	var half = 5 / 2   # integer division
	print(half)
`, null);

// Diagnostics come back as a native JS array — no JSON.parse.
console.log(az.diagnostics(uri));
// → [{ code: "INTEGER_DIVISION", severity: "warning", range: { start, end }, message: ... }]
```

Every query returns a **native JS value** (object / array / `null`) — no
`JSON.parse`. Positions are **UTF-8 byte offsets** (see
[Positions](#positions-byte-offsets) below).

---

## The session model

`AnalysisHandle` is a **live, URI-keyed session**. Construct it once, push
documents, then query — the underlying Rust `AnalysisHost` (and its incremental
salsa cache) stays warm across edits, so re-queries after a small edit are cheap.

```js
const az = new AnalysisHandle();

// Open / replace / close documents by URI.
az.openDocument(uri, text, resPath);   // resPath ("res://…") is optional — see below
az.changeDocument(uri, newText);       // replace text (unknown URI ⇒ upsert)
az.closeDocument(uri);
az.isOpen(uri);                        // → boolean

// Optional project context (enables [autoload] singleton resolution).
az.setProjectConfig(projectGodotText);
```

### Cross-file resolution

Pass each document's `res://` path on **first open** to enable cross-file
`preload(...)`, `extends "res://…"`, and autoload resolution:

```js
az.openDocument("inmemory://player.gd", playerSrc, "res://entities/player.gd");
az.openDocument("inmemory://enemy.gd",  enemySrc,  "res://entities/enemy.gd");
// Now hover/goto across the two files resolves correctly.
```

`resPath` is recorded once and ignored on later `openDocument` calls for the same
URI (re-sending would needlessly invalidate the resource-path registry). Use
`changeDocument` for edits.

---

## API

All queries take a `uri`; offset-based queries take a UTF-8 **byte** `offset`.
Array/object queries return a **native JS value**; the `… | null` ones return JS
`null` when there's nothing at the offset. Navigation/edit results
(`gotoDefinition`, `findReferences`, `rename`) carry a `uri` per target, so you
need no `FileId`→URI mapping of your own.

| Method | Returns | What |
| --- | --- | --- |
| `diagnostics(uri)` | array | parse + type diagnostics |
| `documentSymbols(uri)` | array | the document outline |
| `foldingRanges(uri)` | array | foldable ranges |
| `inlayHints(uri)` | array | inferred-type / param inlay hints |
| `completions(uri, offset)` | array | completions at `offset` |
| `hover(uri, offset)` | object \| `null` | type + docs at `offset` |
| `signatureHelp(uri, offset)` | object \| `null` | active call signature |
| `codeActions(uri, offset)` | array | quick fixes at `offset` |
| `gotoDefinition(uri, offset)` | array | definition target(s) (each with a `uri`) |
| `findReferences(uri, offset)` | array | all references (each with a `uri`) |
| `rename(uri, offset, newName)` | object | `{ ok: SourceChange }` or `{ error: RenameError }` |
| `workspaceSymbols(query)` | array | project-wide symbol search |
| `syntaxTree(uri)` | string \| `null` | pretty-printed CST (debugging) |

```js
const offset = 38; // a UTF-8 byte offset into the document
az.completions(uri, offset);                  // array
const hover = az.hover(uri, offset);          // object | null
if (hover) console.log(hover);
az.gotoDefinition(uri, offset);               // array, each target has a `uri`
```

### Positions (byte offsets)

The analyzer speaks **UTF-8 byte offsets**, not line/column or UTF-16 code units.
JavaScript strings are UTF-16, so convert at the boundary:

```js
const enc = new TextEncoder();
// UTF-16 index (e.g. from an editor) → UTF-8 byte offset
const byteOffset = enc.encode(text.slice(0, utf16Index)).length;
```

If you're building an LSP/editor integration, the diagnostic/hover/definition
`range`s come back as byte offsets too — convert them back to your editor's
position type on the way out.

---

## Notes

- **No Godot required.** Analysis is fully static; it never launches or talks to a
  Godot editor.
- **Single-threaded ownership.** An `AnalysisHandle` is owned by the JS thread
  that created it; don't share one instance across worker threads (create one per
  thread instead).
- **Engine model.** The Godot 4.x class/method/signal model is embedded in the
  native binary — no extra asset to load (unlike the wasm package).

## Links

- **Repository & docs:** https://github.com/yanivkalfa/gdscript-analyzer
- **Live playground:** https://yanivkalfa.github.io/gdscript-analyzer/playground/
- **Browser package:** [`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)
- **License:** MIT OR Apache-2.0
