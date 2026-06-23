# Known divergences — our parser vs. tree-sitter-gdscript

The differential oracle ([`tests/differential.rs`](differential.rs)) cross-validates our
hand-written parser against [`tree-sitter-gdscript`](https://github.com/PrestonKnopp/tree-sitter-gdscript)
(v6.1.0). tree-sitter is the **reference grammar and oracle only** — never our
grammar-of-record (single external maintainer, manually synced to Godot; see
ADR-0002 / `plans/research/02-parsing-strategy.md`).

Run it with:

```
cargo test -p gdscript-syntax --features tree-sitter-oracle --test differential
# or:
cargo xtask differential
```

## Current status

**No divergences on the core corpus.** On the common GDScript surface (functions,
classes, vars/consts, signals, enums, control flow, `match`, expressions with full
precedence, lambdas, inner classes, annotations) our parser and tree-sitter **agree**
on whether a file is well-formed.

## Intentionally excluded from the differential corpus

These are not divergences to fix — they are inputs we deliberately keep out of the
oracle corpus because the two parsers are expected to differ:

| Area | Why excluded |
|---|---|
| **Typed dictionaries** `Dictionary[K, V]` (Godot 4.4+) | tree-sitter 6.1 may not model the newer generic-dictionary syntax; our parser accepts it permissively. |
| **Varargs** `func f(...rest):` (Godot 4.5+) | Recent syntax tree-sitter may lag on. |
| **`@abstract`** classes/methods (Godot 4.5+) | Recent annotation; lexed as `@` + `abstract` identifier by us. |
| **Indentation tab width** | Our pre-pass uses Godot's flat `tab_size = 4`; tree-sitter's `scanner.c` hardcodes 8-column tab stops. This only matters on mixed/irregular indentation (which Godot itself errors on), so it never affects well-formed files — but it is the wrong oracle for indentation column math. |

## Triage policy

When the differential test reports a divergence on input that *should* be valid (or
invalid) GDScript:

1. **If our parser is wrong** — fix the grammar; add the case as a regular parser test.
2. **If tree-sitter is wrong / outdated** — record the input here with a short
   rationale and exclude it from the corpus (or add an allowlist entry).

The Godot engine's own `modules/gdscript` tokenizer + parser is the ultimate source of
truth; tree-sitter and our parser are both approximations of it.
