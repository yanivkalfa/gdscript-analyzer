//! The normalized in-memory Godot engine model — the shape of `extension_api.json` after
//! `xtask` has resolved its asymmetries (Playbook §4.1/§4.2).
//!
//! [`ApiData`] is the serializable root: a set of flat tables (classes, builtins, …) keyed
//! by integer ids ([`ClassId`]/[`BuiltinId`]). It is what `xtask codegen-api` `rkyv`-encodes
//! into the bundled blob, and what [`crate::EngineApi`] deserializes and indexes at load.
//!
//! Strings are owned (`String`) rather than interned here: the model is built once and
//! `Arc`-shared, its names are read by reference and never cloned on a hot path, so interning
//! buys nothing — `SmolStr` is reserved for `gdscript-hir`, where source names *are* cloned
//! and compared. Keeping the archived form free of custom string/hash types also keeps the
//! `rkyv` blob trivially portable.

// `rkyv`'s derive emits public `Archived*` companion types we never name; allowing the
// missing-Debug lint here keeps it on everywhere else. Our own owned types still derive Debug.
#![allow(missing_debug_implementations)]

use rkyv::{Archive, Deserialize, Serialize};

/// Index of an engine class in [`ApiData::classes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, Serialize, Deserialize)]
pub struct ClassId(pub u32);

/// Index of a builtin (Variant) type in [`ApiData::builtins`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, Serialize, Deserialize)]
pub struct BuiltinId(pub u32);

/// Index into the [`DocStore`] documentation table. A symbol with no doc text has `doc: None`;
/// every populated handle addresses a non-empty Markdown entry (Playbook §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, Serialize, Deserialize)]
pub struct DocId(pub u32);

/// The engine documentation store: per-symbol Markdown doc text addressed by [`DocId`].
///
/// Encoded into a **separate** `engine_docs.bin` blob (deliberately *not* a field of [`ApiData`])
/// and embedded native-only behind the `bundled-docs` feature. This keeps the doc prose out of the
/// `engine_api.bin` blob the wasm playground `fetch`es — populating the `doc: Option<DocId>` fields
/// is a fixed-size change to that blob, so the wasm download never grows with the docs (Playbook
/// §4.6; `crates/gdscript-api/src/lib.rs`).
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct DocStore {
    /// Markdown entries; `DocId(i)` addresses `entries[i]`. Every entry is non-empty — a symbol
    /// with no documentation carries `doc: None`, never an index to an empty string.
    pub entries: Vec<String>,
}

impl DocStore {
    /// The Markdown for a doc handle, or `None` if the index is out of range.
    #[must_use]
    pub fn get(&self, id: DocId) -> Option<&str> {
        self.entries.get(id.0 as usize).map(String::as_str)
    }
}

/// The Godot version the model was generated from.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct ApiVersion {
    /// Major version (e.g. `4`).
    pub major: u32,
    /// Minor version (e.g. `5`).
    pub minor: u32,
    /// Patch version (e.g. `0`).
    pub patch: u32,
    /// Release status (e.g. `stable`).
    pub status: String,
}

/// Whether a symbol is part of the runtime API or editor-only (Playbook §4.2 — gate
/// editor-only symbols out of runtime completion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum ApiType {
    /// Available at runtime.
    Core,
    /// Editor-only.
    Editor,
}

/// The element type of a typed container. Non-recursive on purpose: Phase 2 does not track
/// nested element types, so a nested typed container (`Array[Array[int]]`) collapses to its
/// bare builtin (`Array`) here (Playbook §2). Keeping this flat also keeps the `rkyv` archive
/// free of recursive `Box` bounds.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum ElemRef {
    /// The dynamic `Variant` top type (a bare container's element).
    Variant,
    /// A builtin Variant type (includes bare `Array`/`Dictionary` collapsed from nesting).
    Builtin(BuiltinId),
    /// An engine class.
    Class(ClassId),
    /// `enum::…` / `bitfield::…` — kept qualified.
    Enum {
        /// The dotted name as written after `enum::`/`bitfield::`.
        qualified: String,
        /// Whether the source prefix was `bitfield::`.
        bitfield: bool,
    },
}

/// An unresolved API type reference, parsed from the `extension_api.json` type-string grammar
/// (Playbook §4.2). `Builtin`/`Class` are already resolved to ids at codegen time (second
/// pass, after the name tables are built); `Enum` keeps its qualified string for `gdscript-hir`
/// to resolve against a class's / the global enum set.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum TyRef {
    /// No value (`void` return).
    Void,
    /// The dynamic `Variant` top type.
    Variant,
    /// A builtin Variant type.
    Builtin(BuiltinId),
    /// An engine class.
    Class(ClassId),
    /// `typedarray::T` → `Array[T]`.
    TypedArray(ElemRef),
    /// `typeddictionary::K::V` → `Dictionary[K, V]`.
    TypedDict(ElemRef, ElemRef),
    /// `enum::Class.Enum`, `enum::GlobalEnum`, or `bitfield::…` — kept qualified.
    Enum {
        /// The dotted name as written after `enum::`/`bitfield::`.
        qualified: String,
        /// Whether the source prefix was `bitfield::`.
        bitfield: bool,
    },
}

/// A function/method parameter.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct Param {
    /// The parameter name.
    pub name: String,
    /// The parameter type.
    pub ty: TyRef,
    /// The default-value **source string** (e.g. `"Vector2(0, 0)"`), displayed verbatim and
    /// never evaluated (Playbook §4.2). `None` when the parameter is required.
    pub default: Option<String>,
}

/// A method signature (engine class method, builtin method, or — without the receiver — a
/// utility function shares the same shape via [`UtilityFn`]).
// The four `is_*` flags faithfully mirror the engine's per-method flags; folding them into a
// bitfield would only obscure that one-to-one mapping.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct MethodSig {
    /// The method name.
    pub name: String,
    /// The parameters, in order.
    pub params: Vec<Param>,
    /// The return type (`Void` when the source had no return field).
    pub return_ty: TyRef,
    /// `const` (does not mutate the receiver).
    pub is_const: bool,
    /// A `static` method (callable on the type).
    pub is_static: bool,
    /// Accepts a variable number of trailing arguments.
    pub is_vararg: bool,
    /// A virtual method (a `_`-prefixed hook the user overrides).
    pub is_virtual: bool,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// A class property. `enum_of` carries the qualified enum name recovered from the property's
/// getter (Playbook §4.2 — the JSON reports an enum property's storage type as `int`, but its
/// getter's return type is `enum::…`).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct PropertyInfo {
    /// The property name.
    pub name: String,
    /// The storage type as reported by the JSON (`int` for enum properties).
    pub ty: TyRef,
    /// The setter method name, if any.
    pub setter: Option<String>,
    /// The getter method name, if any.
    pub getter: Option<String>,
    /// The qualified enum name when this property is actually enum-typed (from the getter).
    pub enum_of: Option<String>,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// A signal declaration.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct SignalSig {
    /// The signal name.
    pub name: String,
    /// The signal parameters, in order.
    pub params: Vec<Param>,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// One named enum value.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct EnumValue {
    /// The value name (e.g. `SIDE_LEFT`).
    pub name: String,
    /// The integer value.
    pub value: i64,
}

/// An enum (class enum, builtin enum, or global enum).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct EnumInfo {
    /// The enum name (unqualified).
    pub name: String,
    /// Whether the enum is a bitfield (flags).
    pub is_bitfield: bool,
    /// The enum values, in declaration order.
    pub values: Vec<EnumValue>,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// A constant. Engine-class constants are integers (notification/flag values); builtin
/// constants carry a typed source-literal expression (`Vector2(0, 0)`).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct ConstInfo {
    /// The constant name.
    pub name: String,
    /// The constant type (`Builtin(int)` for engine-class integer constants).
    pub ty: TyRef,
    /// The integer value, for engine-class constants.
    pub int_value: Option<i64>,
    /// The source-literal expression, for builtin constants (displayed verbatim).
    pub value_expr: Option<String>,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// An engine class and its members.
#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct ClassData {
    /// The class name.
    pub name: String,
    /// The resolved base class, if any (only `Object` has none).
    pub base: Option<ClassId>,
    /// Whether instances are reference-counted (`RefCounted` subtree).
    pub is_refcounted: bool,
    /// Whether the class can be instantiated directly.
    pub is_instantiable: bool,
    /// Runtime vs. editor-only.
    pub api_type: ApiType,
    /// Declared methods (not including inherited).
    pub methods: Vec<MethodSig>,
    /// Declared properties.
    pub properties: Vec<PropertyInfo>,
    /// Declared signals.
    pub signals: Vec<SignalSig>,
    /// Nested enums.
    pub enums: Vec<EnumInfo>,
    /// Integer constants.
    pub constants: Vec<ConstInfo>,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// One field of a builtin Variant type (e.g. `Vector2.x`).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct BuiltinMember {
    /// The member name.
    pub name: String,
    /// The member type.
    pub ty: TyRef,
}

/// A builtin-type operator overload. `right` is `None` for unary operators (`unary-`, `not`).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct OperatorSig {
    /// The operator token as the JSON spells it (`+`, `==`, `unary-`, `and`, …).
    pub op: String,
    /// The right-hand operand type, or `None` for a unary operator.
    pub right: Option<TyRef>,
    /// The result type.
    pub result: TyRef,
}

/// A builtin (Variant) type and its members.
#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct BuiltinData {
    /// The builtin type name (`Vector2`, `String`, …).
    pub name: String,
    /// Named fields (`Vector2.x`).
    pub members: Vec<BuiltinMember>,
    /// Methods.
    pub methods: Vec<MethodSig>,
    /// Named constants (`Vector2.ZERO`).
    pub constants: Vec<ConstInfo>,
    /// Nested enums (`Vector2.Axis`).
    pub enums: Vec<EnumInfo>,
    /// Operator overloads.
    pub operators: Vec<OperatorSig>,
    /// The element type yielded by `[]` indexing, if the type is indexable.
    pub indexing_return: Option<TyRef>,
    /// Whether the type is keyed (dictionary-like indexing).
    pub is_keyed: bool,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// A `@GlobalScope` utility function (`sin`, `print`, `range`, …).
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct UtilityFn {
    /// The function name.
    pub name: String,
    /// The parameters, in order.
    pub params: Vec<Param>,
    /// The return type.
    pub return_ty: TyRef,
    /// Accepts a variable number of trailing arguments.
    pub is_vararg: bool,
    /// The JSON `category` (`math`, `random`, `general`).
    pub category: String,
    /// Documentation handle, when the doc store is populated.
    pub doc: Option<DocId>,
}

/// The serializable engine-model root: flat tables addressed by [`ClassId`]/[`BuiltinId`].
/// `xtask codegen-api` builds this from `extension_api.json` and `rkyv`-encodes it; the name
/// indices are rebuilt at load by [`crate::EngineApi`], so they are intentionally absent here.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct ApiData {
    /// The source Godot version.
    pub version: ApiVersion,
    /// All engine classes, in `extension_api.json` order (alphabetical).
    pub classes: Vec<ClassData>,
    /// All builtin Variant types.
    pub builtins: Vec<BuiltinData>,
    /// Singletons: `(symbol name, the class it is an instance of)`.
    pub singletons: Vec<(String, ClassId)>,
    /// `@GlobalScope` utility functions.
    pub utilities: Vec<UtilityFn>,
    /// Global (`@GlobalScope`) enums.
    pub global_enums: Vec<EnumInfo>,
}
