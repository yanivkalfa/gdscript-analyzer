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
    /// Bare `Dictionary`.
    Dictionary,
    /// `Color`.
    Color,
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
    use LayerTy::{Array, Bool, Color, Dictionary, Int, Str, Unknown, Variant, Void};
    vec![
        // Two `@GlobalScope` utility functions the extracted blob omits (the extraction filtered
        // them; they are real bare globals — the corpus calls both). Hand-authored here, exactly
        // what this layer is for, so bare calls resolve and never false-flag as UNDEFINED.
        BuiltinFn {
            name: "Color8",
            min_args: 3,
            max_args: Some(4),
            ret: Color,
        },
        BuiltinFn {
            name: "is_instance_of",
            min_args: 2,
            max_args: Some(2),
            ret: Bool,
        },
        // The REST of the `@GDScript` pseudo-class surface (vendor .../doc/classes/@GDScript.xml
        // lists 15 methods; these complete the set). They exist in no extracted table — with the
        // A1 `UNDEFINED_FUNCTION` armed, an uncovered one false-flags common code (`print_debug`).
        BuiltinFn {
            name: "convert",
            min_args: 2,
            max_args: Some(2),
            ret: Variant,
        },
        BuiltinFn {
            name: "dict_to_inst",
            min_args: 1,
            max_args: Some(1),
            ret: Variant,
        },
        BuiltinFn {
            name: "get_stack",
            min_args: 0,
            max_args: Some(0),
            ret: Array,
        },
        BuiltinFn {
            name: "inst_to_dict",
            min_args: 1,
            max_args: Some(1),
            ret: Dictionary,
        },
        BuiltinFn {
            name: "ord",
            min_args: 1,
            max_args: Some(1),
            ret: Int,
        },
        BuiltinFn {
            name: "print_debug",
            min_args: 0,
            max_args: None,
            ret: Void,
        },
        BuiltinFn {
            name: "print_stack",
            min_args: 0,
            max_args: Some(0),
            ret: Void,
        },
        BuiltinFn {
            name: "type_exists",
            min_args: 1,
            max_args: Some(1),
            ret: Bool,
        },
        // `preload(path)` resolves to a script/resource — opaque in Phase 2 (the seam).
        BuiltinFn {
            name: "preload",
            min_args: 1,
            max_args: Some(1),
            ret: Unknown,
        },
        // `load(path)` returns a `Resource` at runtime, but the concrete script/resource type is
        // unknowable statically (the arg may be a variable, and even a literal is a *runtime*
        // call — NOT a compile-time constant like `preload`). Model it as the seam (`Unknown`) so
        // `var r := load(...)` neither warns (`INFERENCE_ON_VARIANT`) nor cascades, and `load` is
        // never aliased to `preload` (Playbook §3.M3 / D5 — both literal and variable args opaque).
        BuiltinFn {
            name: "load",
            min_args: 1,
            max_args: Some(1),
            ret: Unknown,
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
