//! `gdscript-hir` — the semantic / type layer.
//!
//! Lowers the AST to a HIR (an `ItemTree` of signatures + per-function `Body`), runs name resolution
//! (local → class member → inherited → global), gradual type inference (Variant by default,
//! `:=`/annotations, member lookup over the engine inheritance table, `is`/`as` narrowing), and the
//! GDScript warning checks. Single-file in Phase 2; project-wide + scene-aware later.
//!
//! Phase 2 builds this out bottom-up: the type model ([`ty`]), then the item tree, body, name
//! resolution, and inference. Must build for `wasm32`.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod body;
mod cst;
pub mod def;
pub mod infer;
pub mod item_tree;
pub mod project;
pub mod queries;
pub mod resolve;
pub mod ty;

pub use cst::AstPtr;
