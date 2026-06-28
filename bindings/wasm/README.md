# @gdscript-analyzer/wasm

**Browser/WebAssembly binding for [gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer) — a fast, embeddable GDScript (Godot 4.x) static-analysis library. "Roslyn for Godot."**

[![npm](https://img.shields.io/npm/v/@gdscript-analyzer/wasm?logo=npm)](https://www.npmjs.com/package/@gdscript-analyzer/wasm)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/yanivkalfa/gdscript-analyzer#license)

Run a full GDScript analyzer **entirely in the browser** — no server, no Godot,
no network: diagnostics, type-aware hover, completion, symbols, and navigation.
This is the `wasm-bindgen` (`--target web`, ESM) build; for Node use the native
[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core).

**▶︎ See it running:** [the playground](https://yanivkalfa.github.io/gdscript-analyzer/playground/).

```sh
npm i @gdscript-analyzer/wasm
```

---

## Quick start

```js
import init, { Analyzer } from "@gdscript-analyzer/wasm";

await init();                       // instantiate the .wasm (required, once)
const az = new Analyzer();

const uri = "inmemory://main.gd";
az.openDocument(uri, "extends Node\nfunc _ready():\n\tvar x = 5 / 2\n", null);

console.log(az.diagnostics(uri));  // → INTEGER_DIVISION warning (a native JS array, no JSON.parse)
```

Bundlers and native ESM both work. With Vite/webpack, `import init` resolves the
`.wasm` automatically; for a no-bundler setup, serve the package and
`import init from "/node_modules/@gdscript-analyzer/wasm/gdscript.js"`.

---

## Engine-class completion (optional)

Parse + type diagnostics work out of the box. To get completion/hover for **Godot
engine classes** (`Node`, `Button`, …), load the engine model once — a binary
blob you host alongside your app:

```js
const bytes = new Uint8Array(await (await fetch("/data/extension_api.bin")).arrayBuffer());
az.loadEngineApi(bytes);            // → true on success
```

The blob is the analyzer's compiled Godot 4.x API model. The
[playground](https://yanivkalfa.github.io/gdscript-analyzer/playground/) ships one
at `playground/data/extension_api.bin` you can copy, or generate it from the repo.

---

## API

Same URI-keyed session model as the native package. Construct once, push
documents, query by **UTF-8 byte offset**; queries return **native JS values**
(no `JSON.parse`). Navigation/edit results (`gotoDefinition`, `findReferences`,
`rename`) carry a `uri` per target, so you need no `FileId`→URI mapping of your own.

```js
az.openDocument(uri, text, resPath);   // resPath ("res://…") enables cross-file resolution
az.changeDocument(uri, newText);
az.closeDocument(uri);
az.setProjectConfig(projectGodotText); // enables [autoload] resolution
az.loadEngineApi(bytes);               // optional engine model

az.diagnostics(uri);                   // array
az.documentSymbols(uri);               // array
az.completions(uri, byteOffset);       // array
const hover = az.hover(uri, byteOffset);          // object | null
az.gotoDefinition(uri, byteOffset);    // array, each target has a `uri`
az.inlayHints(uri);                    // array
az.foldingRanges(uri);                 // array
```

### Positions (byte offsets)

The analyzer speaks **UTF-8 byte offsets**; the browser's strings/editors are
UTF-16. Convert at the boundary — the
[playground source](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/playground/index.html)
has a complete, copy-pasteable `utf16 ⇄ byte` helper plus an editor wiring you can
lift directly.

```js
const enc = new TextEncoder();
const byteOffset = enc.encode(text.slice(0, utf16Index)).length;
```

---

## Links

- **Repository & docs:** https://github.com/yanivkalfa/gdscript-analyzer
- **Playground (live demo + reference integration):** https://yanivkalfa.github.io/gdscript-analyzer/playground/
- **Native (Node) package:** [`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)
- **License:** MIT OR Apache-2.0
