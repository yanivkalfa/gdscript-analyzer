// @gdscript-analyzer/core — Node binding entry point.
//
// Phase 0: scaffold only. Phase 1 wires the napi-rs v3 addon (built from crates/gdscript-ffi);
// this loader will then resolve the correct per-platform `@gdscript-analyzer/core-<triple>`
// optionalDependency (swc-style, with musl detection) or the locally-built `.node`, and re-export
// the AnalysisHandle API. See plans/PHASE-5-CLIENTS-AND-DISTRIBUTION.md.

module.exports = {
  __status: "scaffold",
  __note: "The napi-rs binding is wired in Phase 1; this package is not functional yet.",
};
