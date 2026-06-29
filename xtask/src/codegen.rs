//! `extension_api.json` → normalized [`ApiData`] → `rkyv` blob (Playbook §4.2/§4.5).
//!
//! Two stages: permissive `Raw*` serde structs mirror the JSON, then [`normalize`] lowers them
//! into the owned model — resolving `inherits` to ids in a second pass, normalizing the
//! return-type asymmetry (`return_value{type}` vs. flat `return_type`), recovering enum-typed
//! properties from their getter, and parsing the type-string grammar. The result is validated
//! against golden symbols and `rkyv`-encoded. This all runs at codegen time only, so the JSON
//! parser and the model builder never reach `wasm`.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use gdscript_api::{
    ApiData, ApiType, ApiVersion, BuiltinData, BuiltinId, BuiltinMember, ClassData, ClassId,
    ConstInfo, ElemRef, EngineApi, EnumInfo, EnumValue, MemberRef, MethodSig, OperatorSig, Param,
    PropertyInfo, SignalSig, TyRef, UtilityFn,
};

/// The codegen result: the encoded blobs plus summary metadata for `generated.rs`.
pub struct Generated {
    /// The `rkyv`-encoded [`ApiData`] blob (`engine_api.bin`).
    pub blob: Vec<u8>,
    /// The `rkyv`-encoded [`gdscript_api::DocStore`] blob (`engine_docs.bin`) — a *separate*,
    /// native-only artifact so it never grows the wasm-fetched `engine_api.bin`.
    pub docs_blob: Vec<u8>,
    /// Number of distinct doc entries interned into the doc store.
    pub doc_count: usize,
    /// The Godot version string (`4.5.0-stable`).
    pub version: String,
    /// Number of engine classes.
    pub class_count: usize,
    /// Number of builtin types.
    pub builtin_count: usize,
}

/// Parse, normalize, attach docs, validate, and encode `extension_api.json` (+ the doc XML).
pub fn generate(json: &str, docs: &crate::docs::DocSet) -> Result<Generated> {
    let raw: RawApi = serde_json::from_str(json).context("parsing extension_api.json")?;
    let version = format!(
        "{}.{}.{}-{}",
        raw.header.version_major,
        raw.header.version_minor,
        raw.header.version_patch,
        raw.header.version_status,
    );
    let class_count = raw.classes.len();
    let builtin_count = raw.builtin_classes.len();

    let mut api = normalize(&raw);
    // Second pass: tag each symbol with a `DocId` and build the separate doc store.
    let doc_store = crate::docs::attach_docs(&mut api, docs);
    let doc_count = doc_store.entries.len();

    let blob = rkyv::to_bytes::<rkyv::rancor::Error>(&api)
        .map_err(|e| anyhow!("rkyv-encoding the engine model: {e}"))?
        .to_vec();
    let docs_blob = rkyv::to_bytes::<rkyv::rancor::Error>(&doc_store)
        .map_err(|e| anyhow!("rkyv-encoding the doc store: {e}"))?
        .to_vec();

    // Validate the *actual artifacts* round-trip and the golden symbols (incl. a golden doc)
    // resolve, so a bad regen fails loudly here rather than at the analyzer's first query.
    let mut engine = EngineApi::from_bytes(&blob).context("decoding the freshly-encoded blob")?;
    engine.set_docs(
        gdscript_api::DocStore::from_bytes(&docs_blob)
            .map_err(|e| anyhow!("decoding the freshly-encoded doc store: {e}"))?,
    );
    validate_golden(&engine)?;

    Ok(Generated {
        blob,
        docs_blob,
        doc_count,
        version,
        class_count,
        builtin_count,
    })
}

/// Lower the raw JSON into the owned model (the two-pass normalize).
fn normalize(raw: &RawApi) -> ApiData {
    // Pass 1 — name → id (alphabetical JSON order is positional; ids are the index).
    let builtins_by_name: HashMap<&str, BuiltinId> = raw
        .builtin_classes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), BuiltinId(idx(i))))
        .collect();
    let classes_by_name: HashMap<&str, ClassId> = raw
        .classes
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), ClassId(idx(i))))
        .collect();
    let resolver = Resolver {
        builtins: &builtins_by_name,
        classes: &classes_by_name,
        array: builtins_by_name.get("Array").copied(),
        dict: builtins_by_name.get("Dictionary").copied(),
    };
    let int_ty = builtins_by_name
        .get("int")
        .copied()
        .map_or(TyRef::Variant, TyRef::Builtin);

    // Pass 2 — resolve everything against the id tables.
    let classes = raw
        .classes
        .iter()
        .map(|c| normalize_class(c, &resolver, &classes_by_name, &int_ty))
        .collect();
    let builtins = raw
        .builtin_classes
        .iter()
        .map(|b| normalize_builtin(b, &resolver))
        .collect();
    let utilities = raw
        .utility_functions
        .iter()
        .map(|u| UtilityFn {
            name: u.name.clone(),
            params: params(&u.arguments, &resolver),
            return_ty: u
                .return_type
                .as_deref()
                .map_or(TyRef::Void, |t| resolver.ty(t)),
            is_vararg: u.is_vararg,
            category: u.category.clone(),
            doc: None,
        })
        .collect();
    let global_enums = raw.global_enums.iter().map(enum_info).collect();
    let singletons = raw
        .singletons
        .iter()
        .filter_map(|s| {
            classes_by_name
                .get(s.ty.as_str())
                .map(|id| (s.name.clone(), *id))
        })
        .collect();

    ApiData {
        version: ApiVersion {
            major: raw.header.version_major,
            minor: raw.header.version_minor,
            patch: raw.header.version_patch,
            status: raw.header.version_status.clone(),
        },
        classes,
        builtins,
        singletons,
        utilities,
        global_enums,
    }
}

fn normalize_class(
    c: &RawClass,
    r: &Resolver,
    classes_by_name: &HashMap<&str, ClassId>,
    int_ty: &TyRef,
) -> ClassData {
    // Getter return types, for recovering enum-typed properties (Playbook §4.2).
    let getter_returns: HashMap<&str, &str> = c
        .methods
        .iter()
        .filter_map(|m| {
            m.return_value
                .as_ref()
                .map(|rv| (m.name.as_str(), rv.ty.as_str()))
        })
        .collect();

    ClassData {
        name: c.name.clone(),
        base: c
            .inherits
            .as_deref()
            .and_then(|n| classes_by_name.get(n).copied()),
        is_refcounted: c.is_refcounted,
        is_instantiable: c.is_instantiable,
        api_type: if c.api_type == "editor" {
            ApiType::Editor
        } else {
            ApiType::Core
        },
        methods: c.methods.iter().map(|m| class_method(m, r)).collect(),
        properties: c
            .properties
            .iter()
            .map(|p| PropertyInfo {
                name: p.name.clone(),
                ty: r.ty(&p.ty),
                setter: non_empty(p.setter.as_ref()),
                getter: non_empty(p.getter.as_ref()),
                enum_of: p
                    .getter
                    .as_deref()
                    .and_then(|g| getter_returns.get(g).copied())
                    .and_then(qualified_enum),
                doc: None,
            })
            .collect(),
        signals: c
            .signals
            .iter()
            .map(|s| SignalSig {
                name: s.name.clone(),
                params: params(&s.arguments, r),
                doc: None,
            })
            .collect(),
        enums: c.enums.iter().map(enum_info).collect(),
        constants: c
            .constants
            .iter()
            .map(|k| ConstInfo {
                name: k.name.clone(),
                ty: int_ty.clone(),
                int_value: Some(k.value),
                value_expr: None,
                doc: None,
            })
            .collect(),
        doc: None,
    }
}

fn normalize_builtin(b: &RawBuiltin, r: &Resolver) -> BuiltinData {
    BuiltinData {
        name: b.name.clone(),
        members: b
            .members
            .iter()
            .map(|m| BuiltinMember {
                name: m.name.clone(),
                ty: r.ty(&m.ty),
            })
            .collect(),
        methods: b
            .methods
            .iter()
            .map(|m| MethodSig {
                name: m.name.clone(),
                params: params(&m.arguments, r),
                return_ty: m.return_type.as_deref().map_or(TyRef::Void, |t| r.ty(t)),
                is_const: m.is_const,
                is_static: m.is_static,
                is_vararg: m.is_vararg,
                is_virtual: false,
                doc: None,
            })
            .collect(),
        constants: b
            .constants
            .iter()
            .map(|k| ConstInfo {
                name: k.name.clone(),
                ty: r.ty(&k.ty),
                int_value: None,
                value_expr: Some(k.value.clone()),
                doc: None,
            })
            .collect(),
        enums: b.enums.iter().map(enum_info).collect(),
        operators: b
            .operators
            .iter()
            .map(|o| OperatorSig {
                op: o.name.clone(),
                right: o.right_type.as_deref().map(|t| r.ty(t)),
                result: r.ty(&o.return_type),
            })
            .collect(),
        indexing_return: b.indexing_return_type.as_deref().map(|t| r.ty(t)),
        is_keyed: b.is_keyed,
        doc: None,
    }
}

fn class_method(m: &RawMethod, r: &Resolver) -> MethodSig {
    MethodSig {
        name: m.name.clone(),
        params: params(&m.arguments, r),
        return_ty: m
            .return_value
            .as_ref()
            .map_or(TyRef::Void, |rv| r.ty(&rv.ty)),
        is_const: m.is_const,
        is_static: m.is_static,
        is_vararg: m.is_vararg,
        is_virtual: m.is_virtual,
        doc: None,
    }
}

fn params(args: &[RawArg], r: &Resolver) -> Vec<Param> {
    args.iter()
        .map(|a| Param {
            name: a.name.clone(),
            ty: r.ty(&a.ty),
            default: a.default_value.clone(),
        })
        .collect()
}

fn enum_info(e: &RawEnum) -> EnumInfo {
    EnumInfo {
        name: e.name.clone(),
        is_bitfield: e.is_bitfield,
        values: e
            .values
            .iter()
            .map(|v| EnumValue {
                name: v.name.clone(),
                value: v.value,
            })
            .collect(),
        doc: None,
    }
}

/// `enum::X` / `bitfield::X` → `X`; anything else → `None`.
fn qualified_enum(ty: &str) -> Option<String> {
    ty.strip_prefix("enum::")
        .or_else(|| ty.strip_prefix("bitfield::"))
        .map(str::to_owned)
}

fn non_empty(s: Option<&String>) -> Option<String> {
    s.map(String::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[allow(clippy::cast_possible_truncation)]
fn idx(i: usize) -> u32 {
    i as u32
}

/// Resolves `extension_api.json` type strings to [`TyRef`]/[`ElemRef`] against the id tables.
struct Resolver<'a> {
    builtins: &'a HashMap<&'a str, BuiltinId>,
    classes: &'a HashMap<&'a str, ClassId>,
    array: Option<BuiltinId>,
    dict: Option<BuiltinId>,
}

impl Resolver<'_> {
    /// Parse a top-level type string (Playbook §4.2 grammar).
    fn ty(&self, s: &str) -> TyRef {
        let s = s.trim();
        if s.is_empty() || s == "void" {
            return TyRef::Void;
        }
        if let Some(rest) = s.strip_prefix("typedarray::") {
            return TyRef::TypedArray(self.elem(rest));
        }
        if let Some(rest) = s.strip_prefix("typeddictionary::") {
            let (k, v) = rest.split_once(';').unwrap_or((rest, "Variant"));
            return TyRef::TypedDict(self.elem(k), self.elem(v));
        }
        if let Some(rest) = s.strip_prefix("enum::") {
            return TyRef::Enum {
                qualified: rest.to_owned(),
                bitfield: false,
            };
        }
        if let Some(rest) = s.strip_prefix("bitfield::") {
            return TyRef::Enum {
                qualified: rest.to_owned(),
                bitfield: true,
            };
        }
        if s == "Variant" {
            return TyRef::Variant;
        }
        let name = strip_hint(s);
        if let Some(id) = self.builtins.get(name) {
            return TyRef::Builtin(*id);
        }
        if let Some(id) = self.classes.get(name) {
            return TyRef::Class(*id);
        }
        TyRef::Variant
    }

    /// Parse a typed-container element. Nested typed containers collapse to their bare builtin
    /// (Phase 2 does not track nested element types — Playbook §2).
    fn elem(&self, s: &str) -> ElemRef {
        let s = s.trim();
        if s.starts_with("typedarray::") {
            return self.array.map_or(ElemRef::Variant, ElemRef::Builtin);
        }
        if s.starts_with("typeddictionary::") {
            return self.dict.map_or(ElemRef::Variant, ElemRef::Builtin);
        }
        if let Some(rest) = s.strip_prefix("enum::") {
            return ElemRef::Enum {
                qualified: rest.to_owned(),
                bitfield: false,
            };
        }
        if let Some(rest) = s.strip_prefix("bitfield::") {
            return ElemRef::Enum {
                qualified: rest.to_owned(),
                bitfield: true,
            };
        }
        if s == "Variant" {
            return ElemRef::Variant;
        }
        let name = strip_hint(s);
        if let Some(id) = self.builtins.get(name) {
            return ElemRef::Builtin(*id);
        }
        if let Some(id) = self.classes.get(name) {
            return ElemRef::Class(*id);
        }
        ElemRef::Variant
    }
}

/// `24/17:CompositorEffect` → `CompositorEffect` (the PropertyHint/usage prefix Godot leaks
/// into a few typed-array element strings). Plain names pass through unchanged.
fn strip_hint(s: &str) -> &str {
    s.rsplit(':').next().unwrap_or(s)
}

/// Assert a handful of golden symbols resolve, so a bad regen fails at codegen (Playbook §4.5).
fn validate_golden(api: &EngineApi) -> Result<()> {
    let node = api
        .class_by_name("Node")
        .context("golden: class `Node` is missing")?;
    let add_child = api
        .lookup_member(node, "add_child")
        .context("golden: `Node.add_child` did not resolve")?;

    // Golden doc: the doc store is populated and a well-known method's prose resolved + converted.
    let MemberRef::Method(m) = add_child else {
        bail!("golden: `Node.add_child` is not a method");
    };
    let doc = m
        .doc
        .and_then(|id| api.doc(id))
        .context("golden: `Node.add_child` has no hover doc")?;
    // A residual `[/…]` closing tag means BBCode leaked through the converter (Markdown links
    // `[text](url)` never contain one).
    if doc.contains("[/") {
        bail!("golden: `Node.add_child` doc leaks unconverted BBCode: {doc:?}");
    }
    if api.class(node).doc.and_then(|id| api.doc(id)).is_none() {
        bail!("golden: class `Node` has no hover doc");
    }
    api.class_by_name("Object")
        .context("golden: class `Object` is missing")?;
    api.singleton("Input")
        .context("golden: `Input` is not a singleton")?;

    let vec2 = api
        .builtin_by_name("Vector2")
        .context("golden: builtin `Vector2` is missing")?;
    let plus_vec2 = api.builtin_operators(vec2).iter().any(|o| {
        o.op == "+" && matches!(o.right, Some(TyRef::Builtin(id)) if Some(id) == Some(vec2))
    });
    if !plus_vec2 {
        bail!("golden: `Vector2 + Vector2` operator did not resolve");
    }
    Ok(())
}

// ---- Raw serde mirror of extension_api.json (only the fields we consume) ----

#[derive(Deserialize)]
struct RawApi {
    header: RawHeader,
    #[serde(default)]
    global_enums: Vec<RawEnum>,
    #[serde(default)]
    utility_functions: Vec<RawUtility>,
    #[serde(default)]
    builtin_classes: Vec<RawBuiltin>,
    #[serde(default)]
    classes: Vec<RawClass>,
    #[serde(default)]
    singletons: Vec<RawSingleton>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)] // field names mirror the JSON keys verbatim
struct RawHeader {
    version_major: u32,
    version_minor: u32,
    version_patch: u32,
    version_status: String,
}

#[derive(Deserialize)]
struct RawEnum {
    name: String,
    #[serde(default)]
    is_bitfield: bool,
    values: Vec<RawEnumValue>,
}

#[derive(Deserialize)]
struct RawEnumValue {
    name: String,
    value: i64,
}

#[derive(Deserialize)]
struct RawArg {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    default_value: Option<String>,
}

#[derive(Deserialize)]
struct RawUtility {
    name: String,
    #[serde(default)]
    return_type: Option<String>,
    category: String,
    is_vararg: bool,
    #[serde(default)]
    arguments: Vec<RawArg>,
}

#[derive(Deserialize)]
struct RawBuiltin {
    name: String,
    #[serde(default)]
    is_keyed: bool,
    #[serde(default)]
    indexing_return_type: Option<String>,
    #[serde(default)]
    members: Vec<RawMember>,
    #[serde(default)]
    constants: Vec<RawBuiltinConst>,
    #[serde(default)]
    enums: Vec<RawEnum>,
    #[serde(default)]
    operators: Vec<RawOperator>,
    #[serde(default)]
    methods: Vec<RawBuiltinMethod>,
}

#[derive(Deserialize)]
struct RawMember {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Deserialize)]
struct RawBuiltinConst {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    value: String,
}

#[derive(Deserialize)]
struct RawOperator {
    name: String,
    #[serde(default)]
    right_type: Option<String>,
    return_type: String,
}

#[derive(Deserialize)]
struct RawBuiltinMethod {
    name: String,
    #[serde(default)]
    return_type: Option<String>,
    #[serde(default)]
    is_const: bool,
    #[serde(default)]
    is_static: bool,
    #[serde(default)]
    is_vararg: bool,
    #[serde(default)]
    arguments: Vec<RawArg>,
}

#[derive(Deserialize)]
struct RawClass {
    name: String,
    #[serde(default)]
    is_refcounted: bool,
    #[serde(default)]
    is_instantiable: bool,
    #[serde(default)]
    inherits: Option<String>,
    #[serde(default)]
    api_type: String,
    #[serde(default)]
    enums: Vec<RawEnum>,
    #[serde(default)]
    methods: Vec<RawMethod>,
    #[serde(default)]
    properties: Vec<RawProperty>,
    #[serde(default)]
    signals: Vec<RawSignal>,
    #[serde(default)]
    constants: Vec<RawClassConst>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_excessive_bools)] // mirrors the engine's per-method flags
struct RawMethod {
    name: String,
    #[serde(default)]
    is_const: bool,
    #[serde(default)]
    is_static: bool,
    #[serde(default)]
    is_vararg: bool,
    #[serde(default)]
    is_virtual: bool,
    #[serde(default)]
    return_value: Option<RawReturnValue>,
    #[serde(default)]
    arguments: Vec<RawArg>,
}

#[derive(Deserialize)]
struct RawReturnValue {
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Deserialize)]
struct RawProperty {
    #[serde(rename = "type")]
    ty: String,
    name: String,
    #[serde(default)]
    setter: Option<String>,
    #[serde(default)]
    getter: Option<String>,
}

#[derive(Deserialize)]
struct RawSignal {
    name: String,
    #[serde(default)]
    arguments: Vec<RawArg>,
}

#[derive(Deserialize)]
struct RawClassConst {
    name: String,
    value: i64,
}

#[derive(Deserialize)]
struct RawSingleton {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}
