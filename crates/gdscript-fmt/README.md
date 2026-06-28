# gdscript-fmt

The GDScript source formatter for [`gdscript-analyzer`](https://github.com/yanivkalfa/gdscript-analyzer).

A pure `(source, config) -> source` function — no engine model, no filesystem, `wasm32`-safe.
**Safety first:** it normalizes whitespace (indentation → tabs, trailing whitespace, final
newline) by re-emitting every significant token verbatim, and in `safe_mode` (the default) it
re-lexes its own output and **falls back to the original** if the significant token stream
changed — so it can never alter the meaning of your code, even on input it doesn't fully
understand.

```rust
use gdscript_fmt::{format, FmtConfig};
let tidy = format("func f():\n        return 1\n", &FmtConfig::default());
```

Line-reflow / intra-line spacing (full gdformat parity) is the documented next step; today the
formatter normalizes block indentation and line/file whitespace.
