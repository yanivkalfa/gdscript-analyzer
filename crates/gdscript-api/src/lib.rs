//! `gdscript-api` — the Godot engine model, generated from `extension_api.json`.
//!
//! > **Internal layer (not a stable API).** Depend on [`gdscript-ide`](https://docs.rs/gdscript-ide) (the public surface); the items here
//! > may change between releases.
//!
//! The model (engine classes + inheritance chain, methods, properties, signals, enums,
//! constants, singletons, utility functions, builtin Variant types) plus the hand-authored
//! GDScript layer the dump omits (pseudo-constants + builtin functions). See
//! `plans/PHASE-2-IMPLEMENTATION-PLAYBOOK.md` §4.
//!
//! ## Shape
//! [`model::ApiData`] is the serializable root that `xtask codegen-api` `rkyv`-encodes into a
//! binary blob; [`EngineApi`] deserializes it once, rebuilds the name indices, merges the
//! hand-authored layer, and exposes the lookup API (`lookup.rs`). The model is `Arc`-shared
//! and excluded from per-file timing, so the one-time deserialize is amortized.
//!
//! ## Targets
//! Native builds embed the blob via `include_bytes!` ([`bundled`], behind the default
//! `bundled-api` feature). The crate never touches `std::fs`/clocks/threads, so it builds for
//! `wasm32`; there the blob is **not** embedded (Playbook §4.5) — the host fetches it and calls
//! [`EngineApi::from_bytes`].
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod gdscript_layer;
/// Generated engine-API metadata (version + counts). Produced by `cargo xtask codegen-api`.
pub mod generated;
pub mod lookup;
pub mod model;

use rustc_hash::FxHashMap;

pub use lookup::MemberRef;
pub use model::{
    ApiData, ApiType, ApiVersion, BuiltinData, BuiltinId, BuiltinMember, ClassData, ClassId,
    ConstInfo, DocId, DocStore, ElemRef, EnumInfo, EnumValue, MethodSig, OperatorSig, Param,
    PropertyInfo, SignalSig, TyRef, UtilityFn,
};

/// The Godot version string the bundled engine-API artifact was generated from.
#[must_use]
pub fn godot_version() -> &'static str {
    generated::GODOT_VERSION
}

/// An error loading the engine-API blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The `rkyv` blob failed to validate or decode.
    Decode(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(msg) => write!(f, "failed to decode engine-API blob: {msg}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The loaded, indexed Godot engine model.
///
/// Holds the deserialized [`ApiData`] plus the name → id indices rebuilt at load (kept out of
/// the blob so the archived form stays portable — Playbook §4.5) and the hand-authored
/// GDScript layer (pseudo-constants + builtin functions).
#[derive(Debug)]
pub struct EngineApi {
    pub(crate) data: ApiData,
    pub(crate) class_by_name: FxHashMap<String, ClassId>,
    pub(crate) builtin_by_name: FxHashMap<String, BuiltinId>,
    pub(crate) singleton_by_name: FxHashMap<String, ClassId>,
    pub(crate) utility_by_name: FxHashMap<String, u32>,
    pub(crate) global_enum_by_name: FxHashMap<String, u32>,
    /// Hand-authored `@GlobalScope`/`@GDScript` pseudo-constants (`PI`/`TAU`/`INF`/`NAN`).
    pub(crate) global_consts: Vec<gdscript_layer::GlobalConst>,
    /// Hand-authored GDScript builtin functions (`preload`/`range`/`len`/…).
    pub(crate) gdscript_builtins: Vec<gdscript_layer::BuiltinFn>,
    /// Cached id of the `int` builtin (used to type engine-class integer constants).
    pub(crate) int_builtin: Option<BuiltinId>,
    /// Per-symbol Markdown documentation, when loaded (native `bundled-docs` embed, or a host
    /// `set_docs` call). `None` on `wasm32`/when docs aren't bundled — hover then shows the
    /// signature only, never an error.
    pub(crate) docs: Option<DocStore>,
}

impl EngineApi {
    /// Build the indexed model from a freshly decoded [`ApiData`], rebuilding the name indices
    /// and merging the hand-authored GDScript layer.
    #[must_use]
    pub fn from_data(data: ApiData) -> Self {
        let mut class_by_name = FxHashMap::default();
        for (i, c) in data.classes.iter().enumerate() {
            class_by_name.insert(
                c.name.clone(),
                ClassId(u32::try_from(i).unwrap_or(u32::MAX)),
            );
        }
        let mut builtin_by_name = FxHashMap::default();
        for (i, b) in data.builtins.iter().enumerate() {
            builtin_by_name.insert(
                b.name.clone(),
                BuiltinId(u32::try_from(i).unwrap_or(u32::MAX)),
            );
        }
        let singleton_by_name = data
            .singletons
            .iter()
            .map(|(name, id)| (name.clone(), *id))
            .collect();
        let mut utility_by_name = FxHashMap::default();
        for (i, u) in data.utilities.iter().enumerate() {
            utility_by_name.insert(u.name.clone(), u32::try_from(i).unwrap_or(u32::MAX));
        }
        let mut global_enum_by_name = FxHashMap::default();
        for (i, e) in data.global_enums.iter().enumerate() {
            global_enum_by_name.insert(e.name.clone(), u32::try_from(i).unwrap_or(u32::MAX));
        }
        let int_builtin = builtin_by_name.get("int").copied();

        Self {
            data,
            class_by_name,
            builtin_by_name,
            singleton_by_name,
            utility_by_name,
            global_enum_by_name,
            global_consts: gdscript_layer::global_consts(),
            gdscript_builtins: gdscript_layer::builtin_fns(),
            int_builtin,
            docs: None,
        }
    }

    /// Install a documentation store (the native `bundled-docs` blob, or a host-supplied one).
    pub fn set_docs(&mut self, docs: DocStore) {
        self.docs = Some(docs);
    }

    /// The Markdown documentation for a [`DocId`], if a doc store is loaded and the handle is in
    /// range. Returns `None` when docs aren't available (e.g. `wasm32` without a host `set_docs`),
    /// so every caller degrades to a signature-only hover.
    #[must_use]
    pub fn doc(&self, id: DocId) -> Option<&str> {
        self.docs.as_ref()?.get(id)
    }

    /// Decode and index an engine-API blob produced by `xtask codegen-api`.
    ///
    /// The bytes are copied into a 16-byte-aligned buffer before validation so a misaligned
    /// source (e.g. `include_bytes!` or a `fetch()`ed `ArrayBuffer`) decodes correctly.
    ///
    /// # Errors
    /// Returns [`LoadError::Decode`] if the blob fails `rkyv` validation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LoadError> {
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(bytes);
        let data = rkyv::from_bytes::<ApiData, rkyv::rancor::Error>(aligned.as_slice())
            .map_err(|e| LoadError::Decode(e.to_string()))?;
        Ok(Self::from_data(data))
    }

    /// The Godot version this model was generated from.
    #[must_use]
    pub fn version(&self) -> &ApiVersion {
        &self.data.version
    }
}

impl DocStore {
    /// Decode an `engine_docs.bin` blob produced by `xtask codegen-api`.
    ///
    /// The bytes are copied into a 16-byte-aligned buffer before validation, mirroring
    /// [`EngineApi::from_bytes`], so a misaligned source decodes correctly.
    ///
    /// # Errors
    /// Returns [`LoadError::Decode`] if the blob fails `rkyv` validation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LoadError> {
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(bytes);
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(aligned.as_slice())
            .map_err(|e| LoadError::Decode(e.to_string()))
    }
}

/// The bundled engine-API model, decoded once on first use.
///
/// Native-only and gated on the default `bundled-api` feature: the blob is embedded via
/// `include_bytes!`. On `wasm32` the blob is not embedded — fetch it and use
/// [`EngineApi::from_bytes`] instead (Playbook §4.5).
///
/// # Panics
/// Panics if the embedded blob fails to decode, which can only happen if `engine_api.bin` was
/// hand-edited or truncated — `cargo xtask codegen-api` always emits a valid, self-validated
/// artifact.
#[cfg(all(feature = "bundled-api", not(target_arch = "wasm32")))]
#[must_use]
pub fn bundled() -> &'static EngineApi {
    use std::sync::OnceLock;
    static BUNDLED: OnceLock<EngineApi> = OnceLock::new();
    static BYTES: &[u8] = include_bytes!("engine_api.bin");
    BUNDLED.get_or_init(|| {
        let mut api =
            EngineApi::from_bytes(BYTES).expect("the bundled engine-API blob must be valid");
        // The doc store is a separate, native-only embed (`bundled-docs`): it never enters the
        // wasm-fetched `engine_api.bin`, so the playground download stays lean. A decode failure
        // is non-fatal — hover simply falls back to signature-only.
        #[cfg(feature = "bundled-docs")]
        {
            static DOC_BYTES: &[u8] = include_bytes!("engine_docs.bin");
            if let Ok(docs) = DocStore::from_bytes(DOC_BYTES) {
                api.set_docs(docs);
            }
        }
        api
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_metadata_is_present() {
        // Regenerated by `cargo xtask codegen-api`; the version string is always populated.
        assert!(!crate::generated::GODOT_VERSION.is_empty());
    }

    // The bundled blob is native-only behind the default feature (see `bundled`).
    #[cfg(all(feature = "bundled-api", not(target_arch = "wasm32")))]
    #[test]
    fn bundled_blob_loads_and_resolves_golden_symbols() {
        let api = crate::bundled();

        // Version came through the blob, not just `generated.rs`.
        assert_eq!(api.version().major, 4);
        assert_eq!(api.version().minor, 5);

        // Direct + inherited member resolution and the inheritance walk.
        let node = api.class_by_name("Node").expect("Node class present");
        let node2d = api.class_by_name("Node2D").expect("Node2D class present");
        assert!(api.lookup_member(node, "add_child").is_some());
        assert!(api.is_subclass(node2d, node), "Node2D is a Node");
        assert!(
            api.lookup_member(node2d, "add_child").is_some(),
            "add_child is inherited onto Node2D"
        );

        // The `recv.<TAB>` candidate set includes inherited members, deduped.
        let members = api.members_of(node2d);
        assert!(members.iter().any(|m| m.name() == "add_child"));
        assert!(members.iter().any(|m| m.name() == "position"));

        // Singletons, builtins + operators, the enum-property getter cross-ref.
        assert!(api.singleton("Input").is_some());
        let v2 = api
            .builtin_by_name("Vector2")
            .expect("Vector2 builtin present");
        assert!(api.builtin_member(v2, "x").is_some());
        assert!(api.builtin_operators(v2).iter().any(|o| o.op == "+"));
        let process_mode = api
            .class(node)
            .properties
            .iter()
            .find(|p| p.name == "process_mode")
            .expect("Node.process_mode present");
        assert!(
            process_mode.enum_of.is_some(),
            "process_mode is recovered as enum-typed from its getter"
        );

        // The hand-authored GDScript layer merged at load.
        assert!(api.global_const("PI").is_some());
        assert!(api.gdscript_builtin("preload").is_some());
    }

    // Hover docs are a separate native-only embed (`bundled-docs`).
    #[cfg(all(feature = "bundled-docs", not(target_arch = "wasm32")))]
    #[test]
    fn bundled_docs_resolve_and_are_converted() {
        use crate::MemberRef;
        let api = crate::bundled();
        let node = api.class_by_name("Node").expect("Node class");

        // The class itself and a well-known method carry Markdown hover docs.
        assert!(
            api.class(node).doc.and_then(|id| api.doc(id)).is_some(),
            "Node has a class hover doc"
        );
        let MemberRef::Method(m) = api.lookup_member(node, "add_child").expect("add_child") else {
            panic!("add_child is a method");
        };
        let doc = m
            .doc
            .and_then(|id| api.doc(id))
            .expect("add_child hover doc");
        // BBCode was converted to Markdown — no `[/…]` closing tags survive.
        assert!(!doc.contains("[/"), "BBCode leaked into hover: {doc:?}");
    }
}
