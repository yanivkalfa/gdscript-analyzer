//! The Phase-2 type model (Playbook §2/§3.5): the gradual [`Ty`] lattice over `Variant`, the
//! hard/soft [`TypeSource`], the ported `is_assignable` compatibility check, and `TyRef`→`Ty`
//! resolution against the engine API.
//!
//! GDScript is gradually typed over one runtime value type, `Variant`. Three top-ish types are
//! kept distinct on purpose: [`Ty::Variant`] (the absorbing gradual top), [`Ty::Unknown`] (the
//! Phase-3 cross-file seam — never warns, never cascades, elided from hover), and [`Ty::Error`]
//! (already diagnosed — suppresses further cascade).

use gdscript_api::{BuiltinId, ClassId, ElemRef, EngineApi, TyRef};
use smol_str::SmolStr;

/// An opaque reference to another `.gd` script (by `class_name`/path). Resolved to a concrete
/// type only in Phase 3; in Phase 2 it never appears (the seam returns [`Ty::Unknown`] instead),
/// but the variant exists so the upgrade is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptRefId(pub u32);

/// An interned signal signature id (Phase 3+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignalSigId(pub u32);

/// A reference to an enum type, kept as the qualified name it was written with. Phase 2 does not
/// resolve it to a concrete enum table — `is_assignable` only needs the *kind* (enum values are
/// assignable to `int`), and hover shows the qualified name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumRef {
    /// The dotted name (`Node.ProcessMode`, `Error`, …).
    pub qualified: SmolStr,
    /// Whether the source was a `bitfield::`.
    pub bitfield: bool,
}

/// A Phase-2 type. `Clone` not `Copy` (the `Box`es in `Array`/`Dict`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    /// A builtin Variant type (`int`, `float`, `String`, `Vector2`, …).
    Builtin(BuiltinId),
    /// An engine class instance, this file's own class, or an inner class.
    Object(ClassId),
    /// Another script, opaque in Phase 2 (the seam yields `Unknown` instead).
    ScriptRef(ScriptRefId),
    /// `Array[T]`; a bare `Array` is `Array(Box::new(Ty::Variant))`.
    Array(Box<Ty>),
    /// `Dictionary[K, V]`; a bare `Dictionary` is `Dict(Variant, Variant)`.
    Dict(Box<Ty>, Box<Ty>),
    /// An enum type (an enum value is assignable to `int`).
    Enum(EnumRef),
    /// A `Signal` value.
    Signal(Option<SignalSigId>),
    /// A `Callable` value.
    Callable,
    /// No value (`void`).
    Void,
    /// The gradual top / escape hatch (≈ engine `VARIANT` ≈ Pyright `Any`).
    Variant,
    /// The Phase-3 cross-file seam marker — distinct from `Variant`. Never warns, never appears
    /// in hover, never cascades a diagnostic.
    Unknown,
    /// An already-reported error; suppresses downstream diagnostics.
    Error,
}

impl Ty {
    /// A bare `Array` (`Array[Variant]`).
    #[must_use]
    pub fn array_of_variant() -> Self {
        Self::Array(Box::new(Self::Variant))
    }

    /// A bare `Dictionary` (`Dictionary[Variant, Variant]`).
    #[must_use]
    pub fn dict_of_variant() -> Self {
        Self::Dict(Box::new(Self::Variant), Box::new(Self::Variant))
    }

    /// Whether this is the gradual top `Variant`.
    #[must_use]
    pub fn is_variant(&self) -> bool {
        matches!(self, Self::Variant)
    }

    /// Whether this is the cross-file seam marker.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// Whether this is the already-reported error marker.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }

    /// Whether a diagnostic should be suppressed because this type carries no information
    /// (`Variant`/`Unknown`/`Error`) — the receivers on which `UNSAFE_*` etc. must never fire.
    #[must_use]
    pub fn is_uninformative(&self) -> bool {
        matches!(self, Self::Variant | Self::Unknown | Self::Error)
    }

    /// A display label for hover / inlay hints, or `None` when the type is `Unknown` (elided —
    /// the Phase-3 seam) so we never render a placeholder.
    #[must_use]
    pub fn label(&self, api: &EngineApi) -> Option<String> {
        Some(match self {
            Self::Builtin(id) => api.builtin(*id).name.clone(),
            Self::Object(id) => api.class(*id).name.clone(),
            Self::Array(elem) => match elem.label(api) {
                Some(e) if e != "Variant" => format!("Array[{e}]"),
                _ => "Array".to_owned(),
            },
            Self::Dict(k, v) => match (k.label(api), v.label(api)) {
                (Some(k), Some(v)) if k != "Variant" || v != "Variant" => {
                    format!("Dictionary[{k}, {v}]")
                }
                _ => "Dictionary".to_owned(),
            },
            Self::Enum(e) => e.qualified.to_string(),
            Self::Signal(_) => "Signal".to_owned(),
            Self::Callable => "Callable".to_owned(),
            Self::Void => "void".to_owned(),
            Self::Variant => "Variant".to_owned(),
            // `ScriptRef` (opaque) and the seam/error markers carry no display label.
            Self::ScriptRef(_) | Self::Unknown | Self::Error => return None,
        })
    }
}

/// How a binding's type was established (Playbook §2). The ordering is load-bearing: a type is
/// *hard* (statically enforced) iff its source is greater than [`TypeSource::Inferred`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeSource {
    /// No type known yet.
    Undetected,
    /// Inferred from an initializer (`:=` / soft) — best-effort; downgraded to `Variant` on
    /// conflict rather than erroring.
    Inferred,
    /// Inferred-but-annotated as inferred (`var x := e` once accepted).
    AnnotatedInferred,
    /// Explicitly annotated (`var x: T`) — a mismatch is an error.
    AnnotatedExplicit,
}

impl TypeSource {
    /// A *hard* type is statically enforced (mismatch = error). A *soft* (`Inferred`) type is
    /// best-effort and downgraded to `Variant` on conflict.
    #[must_use]
    pub fn is_hard(self) -> bool {
        self > Self::Inferred
    }
}

/// A typed binding: its [`Ty`] plus how the type was established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedBinding {
    /// The binding's type.
    pub ty: Ty,
    /// How it was established.
    pub source: TypeSource,
}

/// The outcome of [`is_assignable`] — richer than a bool so the caller can raise the right
/// diagnostic (Playbook §3.5/§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assign {
    /// Cleanly assignable.
    Ok,
    /// Assignable, but the source is `Variant` — a gradual (unchecked) escape.
    OkUnsafe,
    /// `float` stored into an `int` slot (`NARROWING_CONVERSION`).
    Narrowing,
    /// `int` used where an enum is expected (`INT_AS_ENUM_WITHOUT_CAST`).
    IntAsEnum,
    /// Not assignable (`TYPE_MISMATCH`).
    No,
}

/// Whether `name` is a `Packed*Array` builtin (`PackedStringArray`, `PackedVector2Array`, …).
fn is_packed_array(name: &str) -> bool {
    name.starts_with("Packed") && name.ends_with("Array")
}

/// Godot's implicit conversions between two **builtin** types (the engine's `Variant::can_convert`
/// for the value-prop slots GDScript accepts silently). Covers the numeric / vector widening +
/// narrowing, `bool`↔`int`, the `String`/`StringName`/`NodePath` family, and `Array`↔`Packed*Array`.
#[allow(
    clippy::unnested_or_patterns,
    reason = "a flat (from, to) conversion table reads more clearly than maximally-nested patterns"
)]
fn builtin_conversion(from: &str, to: &str) -> Assign {
    match (from, to) {
        // Narrowing (NARROWING_CONVERSION, not a hard mismatch): float→int, float-vec→int-vec.
        ("float", "int")
        | ("Vector2", "Vector2i")
        | ("Vector3", "Vector3i")
        | ("Vector4", "Vector4i")
        | ("Rect2", "Rect2i") => Assign::Narrowing,
        // Widening / interchangeable value types Godot converts silently.
        ("int", "float")
        | ("bool", "int")
        | ("bool", "float")
        | ("int", "bool")
        | ("Vector2i", "Vector2")
        | ("Vector3i", "Vector3")
        | ("Vector4i", "Vector4")
        | ("Rect2i", "Rect2")
        | ("String", "StringName")
        | ("String", "NodePath")
        | ("StringName", "String")
        | ("NodePath", "String") => Assign::Ok,
        // A bare `Array` ↔ a `Packed*Array` is a runtime element-checked conversion Godot allows.
        ("Array", t) if is_packed_array(t) => Assign::Ok,
        (f, "Array") if is_packed_array(f) => Assign::Ok,
        _ => Assign::No,
    }
}

/// Whether a value of type `from` may be assigned to a slot of type `to` (the engine's
/// `check_type_compatibility`, ported — Playbook §3.5). **Order matters.**
#[must_use]
pub fn is_assignable(api: &EngineApi, from: &Ty, to: &Ty) -> Assign {
    // 1. Anything assigns to `Variant`.
    if to.is_variant() {
        return Assign::Ok;
    }
    // 2/3. Never cascade through the seam / error markers; a `Variant` source is the gradual
    // escape (allowed-but-unsafe). These precede the structural checks deliberately.
    if matches!(from, Ty::Unknown | Ty::Error) || matches!(to, Ty::Unknown | Ty::Error) {
        return Assign::Ok;
    }
    if from.is_variant() {
        return Assign::OkUnsafe;
    }

    match to {
        Ty::Builtin(to_id) => {
            let to_name = api.builtin(*to_id).name.as_str();
            match from {
                Ty::Builtin(from_id) if from_id == to_id => Assign::Ok,
                Ty::Builtin(from_id) => {
                    builtin_conversion(api.builtin(*from_id).name.as_str(), to_name)
                }
                // An enum value is assignable to `int`.
                Ty::Enum(_) if to_name == "int" => Assign::Ok,
                // A bare/typed `Array` (a `[…]` literal) assigns to any `Packed*Array`: Godot
                // runtime-converts + validates the elements at runtime — never a static mismatch.
                Ty::Array(_) if is_packed_array(to_name) => Assign::Ok,
                _ => Assign::No,
            }
        }
        Ty::Enum(to_enum) => match from {
            Ty::Enum(from_enum) if from_enum == to_enum => Assign::Ok,
            // A *different* enum's value is an `int` at runtime: Godot wants a cast
            // (`INT_AS_ENUM_WITHOUT_CAST`) — never a hard `TYPE_MISMATCH`.
            Ty::Enum(_) => Assign::IntAsEnum,
            // `int` → enum without a cast.
            Ty::Builtin(id) if api.builtin(*id).name == "int" => Assign::IntAsEnum,
            _ => Assign::No,
        },
        Ty::Object(to_class) => match from {
            Ty::Object(from_class) if api.is_subclass(*from_class, *to_class) => Assign::Ok,
            // Downcast (a base value into a derived slot): permitted with a runtime check —
            // unsafe, but not a hard error. Real code relies on `var c: Control = get_child(0)`.
            Ty::Object(from_class) if api.is_subclass(*to_class, *from_class) => Assign::OkUnsafe,
            // A script reference is opaque — treat like the seam, never a mismatch.
            Ty::ScriptRef(_) => Assign::Ok,
            _ => Assign::No,
        },
        // Typed arrays are invariant — but only between two *informative* element types
        // (`Array[Button]` ↛ `Array[Node]`). A bare `Array`/`Array[Variant]`, or an
        // `Array[Unknown]` (cross-file element), assigns freely: the engine permits
        // untyped→typed with a runtime check, and the seam must never hard-error.
        Ty::Array(to_elem) => match from {
            Ty::Array(from_elem)
                if from_elem == to_elem
                    || from_elem.is_uninformative()
                    || to_elem.is_uninformative() =>
            {
                Assign::Ok
            }
            _ => Assign::No,
        },
        Ty::Dict(to_k, to_v) => match from {
            Ty::Dict(from_k, from_v)
                if (from_k == to_k || from_k.is_uninformative() || to_k.is_uninformative())
                    && (from_v == to_v || from_v.is_uninformative() || to_v.is_uninformative()) =>
            {
                Assign::Ok
            }
            _ => Assign::No,
        },
        Ty::Signal(_) => {
            if matches!(from, Ty::Signal(_)) {
                Assign::Ok
            } else {
                Assign::No
            }
        }
        Ty::Callable => {
            if matches!(from, Ty::Callable) {
                Assign::Ok
            } else {
                Assign::No
            }
        }
        Ty::Void => {
            if matches!(from, Ty::Void) {
                Assign::Ok
            } else {
                Assign::No
            }
        }
        // An opaque script-ref target, and the `Variant`/`Unknown`/`Error` targets already
        // handled above, all accept anything.
        Ty::ScriptRef(_) | Ty::Variant | Ty::Unknown | Ty::Error => Assign::Ok,
    }
}

/// Resolve an engine-API [`TyRef`] (the unresolved form stored in the model) to a [`Ty`].
#[must_use]
pub fn resolve_tyref(api: &EngineApi, tyref: &TyRef) -> Ty {
    match tyref {
        TyRef::Void => Ty::Void,
        TyRef::Variant => Ty::Variant,
        // `Array`/`Dictionary`/`Callable`/`Signal` are engine builtins, but we keep dedicated
        // `Ty` variants for them (a lambda is `Ty::Callable`, `[]` is `Ty::Array`); normalize
        // the bare builtin form so annotations, constructors, and values all agree.
        TyRef::Builtin(id) => match api.builtin(*id).name.as_str() {
            "Callable" => Ty::Callable,
            "Signal" => Ty::Signal(None),
            "Array" => Ty::array_of_variant(),
            "Dictionary" => Ty::dict_of_variant(),
            _ => Ty::Builtin(*id),
        },
        TyRef::Class(id) => Ty::Object(*id),
        TyRef::TypedArray(elem) => Ty::Array(Box::new(resolve_elemref(api, elem))),
        TyRef::TypedDict(k, v) => Ty::Dict(
            Box::new(resolve_elemref(api, k)),
            Box::new(resolve_elemref(api, v)),
        ),
        TyRef::Enum {
            qualified,
            bitfield,
        } => Ty::Enum(EnumRef {
            qualified: SmolStr::new(qualified),
            bitfield: *bitfield,
        }),
    }
}

/// Resolve a typed-container element [`ElemRef`] to a [`Ty`].
#[must_use]
pub fn resolve_elemref(_api: &EngineApi, elem: &ElemRef) -> Ty {
    match elem {
        ElemRef::Variant => Ty::Variant,
        ElemRef::Builtin(id) => Ty::Builtin(*id),
        ElemRef::Class(id) => Ty::Object(*id),
        ElemRef::Enum {
            qualified,
            bitfield,
        } => Ty::Enum(EnumRef {
            qualified: SmolStr::new(qualified),
            bitfield: *bitfield,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty_of(api: &EngineApi, builtin: &str) -> Ty {
        Ty::Builtin(api.builtin_by_name(builtin).expect("known builtin"))
    }

    #[test]
    fn type_source_hardness() {
        assert!(!TypeSource::Undetected.is_hard());
        assert!(!TypeSource::Inferred.is_hard());
        assert!(TypeSource::AnnotatedInferred.is_hard());
        assert!(TypeSource::AnnotatedExplicit.is_hard());
    }

    #[test]
    fn variant_and_seam_assignability() {
        let api = gdscript_api::bundled();
        let int = ty_of(api, "int");
        // Anything assigns to Variant; Variant into a typed slot is allowed-but-unsafe.
        assert_eq!(is_assignable(api, &int, &Ty::Variant), Assign::Ok);
        assert_eq!(is_assignable(api, &Ty::Variant, &int), Assign::OkUnsafe);
        // The seam and error markers never cascade, in either direction.
        assert_eq!(is_assignable(api, &Ty::Unknown, &int), Assign::Ok);
        assert_eq!(is_assignable(api, &int, &Ty::Unknown), Assign::Ok);
        assert_eq!(is_assignable(api, &Ty::Error, &int), Assign::Ok);
    }

    #[test]
    fn numeric_conversions() {
        let api = gdscript_api::bundled();
        let int = ty_of(api, "int");
        let float = ty_of(api, "float");
        assert_eq!(is_assignable(api, &int, &float), Assign::Ok); // widening, silent
        assert_eq!(is_assignable(api, &float, &int), Assign::Narrowing);
        assert_eq!(is_assignable(api, &int, &int), Assign::Ok);
        let string = ty_of(api, "String");
        assert_eq!(is_assignable(api, &string, &int), Assign::No);
    }

    #[test]
    fn object_subclassing() {
        let api = gdscript_api::bundled();
        let node = Ty::Object(api.class_by_name("Node").unwrap());
        let node2d = Ty::Object(api.class_by_name("Node2D").unwrap());
        // Node2D is a Node (upcast → Ok); a Node into a Node2D slot is a downcast — permitted
        // with a runtime check (unsafe), not a hard mismatch.
        assert_eq!(is_assignable(api, &node2d, &node), Assign::Ok);
        assert_eq!(is_assignable(api, &node, &node2d), Assign::OkUnsafe);
        // An unrelated builtin is a real mismatch.
        let s = ty_of(api, "String");
        assert_eq!(is_assignable(api, &s, &node), Assign::No);
    }

    #[test]
    fn arrays_are_invariant() {
        let api = gdscript_api::bundled();
        let int = ty_of(api, "int");
        let float = ty_of(api, "float");
        let arr_int = Ty::Array(Box::new(int.clone()));
        let arr_int2 = Ty::Array(Box::new(int));
        let arr_float = Ty::Array(Box::new(float));
        assert_eq!(is_assignable(api, &arr_int, &arr_int2), Assign::Ok);
        // No covariance: Array[int] is not assignable to Array[float] even though int->float.
        assert_eq!(is_assignable(api, &arr_int2, &arr_float), Assign::No);
    }

    #[test]
    fn enum_int_bridge() {
        let api = gdscript_api::bundled();
        let int = ty_of(api, "int");
        let e = Ty::Enum(EnumRef {
            qualified: SmolStr::new("Node.ProcessMode"),
            bitfield: false,
        });
        assert_eq!(is_assignable(api, &e, &int), Assign::Ok); // enum -> int
        assert_eq!(is_assignable(api, &int, &e), Assign::IntAsEnum); // int -> enum (warn)
    }

    #[test]
    fn label_elides_unknown() {
        let api = gdscript_api::bundled();
        assert_eq!(Ty::Unknown.label(api), None);
        assert_eq!(ty_of(api, "int").label(api).as_deref(), Some("int"));
        assert_eq!(
            Ty::Array(Box::new(ty_of(api, "int"))).label(api).as_deref(),
            Some("Array[int]")
        );
    }
}
