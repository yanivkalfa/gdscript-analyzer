//! `gdscript-syntax` — lexer + indentation pre-pass + lossless parser for GDScript.
//!
//! Phase 1 fills this in: a `logos` lexer, a hand-written indentation pre-pass
//! (INDENT/DEDENT/NEWLINE), and a hand-written recursive-descent parser producing a
//! lossless [`cstree`] CST plus a typed AST, behind a `Parser` trait
//! (tree-sitter-gdscript is the differential test oracle, never the
//! grammar-of-record — see ADR-0002).
//!
//! The crate "knows nothing about salsa or LSP" (rust-analyzer's `syntax` layer):
//! it is pure text → tree. It must build for `wasm32` — no `std::fs`, no clocks, no
//! threads.
#![cfg_attr(docsrs, feature(doc_cfg))]
// `cstree`'s `#[derive(Syntax)]` expands to a `from_raw` that `transmute`s a `u32`
// discriminant into `SyntaxKind` (an `unsafe` block). That is the only `unsafe` in
// this crate; everything we hand-write stays safe. This mirrors the workspace lint's
// allowance for crates that must wire an external `unsafe` API (here, the CST builder).
#![allow(unsafe_code)]

pub mod ast;
mod lexer;
mod parser;
mod prepass;
mod syntax_kind;

pub use lexer::{RawToken, tokenize};
pub use parser::{Parse, SyntaxError, parse};
pub use prepass::{IndentDiagnostic, run as run_prepass};
pub use syntax_kind::{GdNode, GdToken, SyntaxKind, SyntaxNode};
