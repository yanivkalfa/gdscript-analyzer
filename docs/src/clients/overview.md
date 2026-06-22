# Editor / LSP Client Integration

> **Status: forthcoming.** A standalone LSP server (`gdscript-lsp`) and the
> editor setup matrix land in **Phase 5**
> ([`plans/ROADMAP.md`](../../../plans/ROADMAP.md)). This page is the seed for
> the "add an LSP client" guide and will grow as the server matures — modeled on
> rust-analyzer's "Other Editors" documentation.

Because gdscript-analyzer is a [library, not a server](../adr/0001-rust-library-not-server.md),
there are two distinct ways to integrate it into an editor.

## Option A — use the standalone LSP server (Phase 5)

`gdscript-lsp` is a real, standalone, spec-compliant Language Server. Unlike the
engine's built-in LSP, it does **not** require a running Godot editor, and it
adds features the engine LSP lacks (semantic tokens, inlay hints, workspace
symbols, rename). You point any LSP-capable editor at the server binary:

- **VS Code** — a thin extension that spawns the server.
- **Neovim** — `nvim-lspconfig` / the built-in client.
- **Helix, Zed, Emacs (eglot/lsp-mode), Sublime (LSP)** — standard LSP client
  configuration pointing at the `gdscript-lsp` executable.

Each editor's concrete setup snippet will live here once the server ships.

## Option B — embed the library directly

If you are building a tool that isn't an editor — a CI checker, a markup
toolchain like guitkx, a web playground, or a custom intelligence feature — you
**embed the library** rather than speaking LSP:

- **Native Rust** → depend on [`gdscript-ide`](../consume/rust.md).
- **Node** → the [napi addon](../consume/node.md) (`@gdscript-analyzer/core`).
- **Browser** → the [wasm package](../consume/browser.md) (`@gdscript-analyzer/wasm`).

This is the path guitkx takes: it needs GDScript intelligence *inside* markup
`{expr}` blocks via a source-map adapter — an analysis need, not an LSP need —
served by the same library.

## What a client is responsible for

Whichever option you choose, the client owns the protocol mapping the library
deliberately stays out of:

- **Byte offsets → UTF-16.** The core emits byte offsets; LSP uses UTF-16 code
  units. Convert at the boundary using the shipped converter.
- **POD codes → protocol shapes.** A `Diagnostic`'s code/severity/range maps to
  your protocol's diagnostic type.
- **Re-issuing cancelled reads.** A concurrent edit cancels in-flight queries;
  the client re-issues.

See [`plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §2 for the
full contract.
