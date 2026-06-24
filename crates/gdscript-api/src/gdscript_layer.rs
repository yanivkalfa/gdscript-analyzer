//! The hand-authored GDScript layer the engine dump omits (Playbook §4.4).
//!
//! `extension_api.json` describes the engine (classes, builtins, `@GlobalScope` utilities) but
//! not the *language* surface GDScript adds on top: the `@GlobalScope`/`@GDScript`
//! pseudo-constants (`PI`/`TAU`/`INF`/`NAN`) and the GDScript builtin functions
//! (`preload`/`load`/`range`/`len`/…), whose return types are decided here rather than read
//! from any dump. `gdscript-hir` consults these during global name resolution.
//!
//! Types are tagged with the coarse, model-independent [`LayerTy`] (resolved to a `gdscript-hir`
//! `Ty` by the consumer) because real [`crate::BuiltinId`]s only exist after the model loads.

/// A coarse type tag for hand-authored symbols, mapped to a `gdscript-hir` `Ty` by the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerTy {
    /// `float`.
    Float,
    /// `int`.
    Int,
    /// `bool`.
    Bool,
    /// `String`.
    Str,
    /// Bare `Array` (`Array[Variant]`).
    Array,
    /// The dynamic `Variant` top type.
    Variant,
    /// The Phase-3 seam marker — distinct from `Variant`, never warns (e.g. `preload`).
    Unknown,
    /// `void`.
    Void,
}

/// A `@GlobalScope`/`@GDScript` pseudo-constant (`PI`, `TAU`, `INF`, `NAN`).
#[derive(Debug, Clone)]
pub struct GlobalConst {
    /// The constant name.
    pub name: &'static str,
    /// Its type.
    pub ty: LayerTy,
}

/// A GDScript builtin function (`preload`, `range`, `len`, …) — distinct from the
/// `@GlobalScope` *utility* functions, which come from the JSON.
#[derive(Debug, Clone)]
pub struct BuiltinFn {
    /// The function name.
    pub name: &'static str,
    /// Minimum argument count.
    pub min_args: u8,
    /// Maximum argument count, or `None` for variadic.
    pub max_args: Option<u8>,
    /// The decided return type. `preload`/`load` are refined by `gdscript-hir` per the
    /// literal-vs-variable argument rule (Playbook §4.4); this is the conservative default.
    pub ret: LayerTy,
}

/// The pseudo-constants `extension_api.json` reports as empty `global_constants` in 4.5.
#[must_use]
pub fn global_consts() -> Vec<GlobalConst> {
    use LayerTy::Float;
    vec![
        GlobalConst {
            name: "PI",
            ty: Float,
        },
        GlobalConst {
            name: "TAU",
            ty: Float,
        },
        GlobalConst {
            name: "INF",
            ty: Float,
        },
        GlobalConst {
            name: "NAN",
            ty: Float,
        },
    ]
}

/// The GDScript builtin functions (the `@GDScript` surface). The list grows as features need
/// it; these are the ones inference and completion rely on in Phase 2.
#[must_use]
pub fn builtin_fns() -> Vec<BuiltinFn> {
    use LayerTy::{Array, Int, Str, Unknown, Variant, Void};
    vec![
        // `preload(path)` resolves to a script/resource — opaque in Phase 2 (the seam).
        BuiltinFn {
            name: "preload",
            min_args: 1,
            max_args: Some(1),
            ret: Unknown,
        },
        // `load(path)` is dynamic; hir narrows `load("literal")` to `Unknown`.
        BuiltinFn {
            name: "load",
            min_args: 1,
            max_args: Some(1),
            ret: Variant,
        },
        BuiltinFn {
            name: "range",
            min_args: 1,
            max_args: Some(3),
            ret: Array,
        },
        BuiltinFn {
            name: "len",
            min_args: 1,
            max_args: Some(1),
            ret: Int,
        },
        BuiltinFn {
            name: "char",
            min_args: 1,
            max_args: Some(1),
            ret: Str,
        },
        BuiltinFn {
            name: "assert",
            min_args: 1,
            max_args: Some(2),
            ret: Void,
        },
    ]
}
