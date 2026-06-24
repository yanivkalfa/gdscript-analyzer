//! The lookup API over [`EngineApi`] (Playbook §4.3): name → id, member resolution with base
//! walks, subclass tests, and the inherited member set for `recv.<TAB>` completion.
//!
//! Everything returns borrowed [`MemberRef`]s into the model — no cloning.

use crate::EngineApi;
use crate::gdscript_layer::{BuiltinFn, GlobalConst};
use crate::model::{
    BuiltinData, BuiltinId, ClassData, ClassId, ConstInfo, EnumInfo, MethodSig, OperatorSig,
    PropertyInfo, SignalSig, UtilityFn,
};
use rustc_hash::FxHashSet;

/// A borrowed reference to a resolved class member (Playbook §4.3 — no clone).
#[derive(Debug, Clone, Copy)]
pub enum MemberRef<'a> {
    /// A method.
    Method(&'a MethodSig),
    /// A property.
    Property(&'a PropertyInfo),
    /// A signal.
    Signal(&'a SignalSig),
    /// A constant.
    Const(&'a ConstInfo),
    /// A nested enum.
    Enum(&'a EnumInfo),
}

impl MemberRef<'_> {
    /// The member's name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Method(m) => &m.name,
            Self::Property(p) => &p.name,
            Self::Signal(s) => &s.name,
            Self::Const(c) => &c.name,
            Self::Enum(e) => &e.name,
        }
    }
}

impl EngineApi {
    // ---- tables ----

    /// All engine classes (`extension_api.json` order).
    #[must_use]
    pub fn classes(&self) -> &[ClassData] {
        &self.data.classes
    }

    /// All builtin Variant types.
    #[must_use]
    pub fn builtins(&self) -> &[BuiltinData] {
        &self.data.builtins
    }

    /// The class with the given id.
    #[must_use]
    pub fn class(&self, id: ClassId) -> &ClassData {
        &self.data.classes[id.0 as usize]
    }

    /// The builtin type with the given id.
    #[must_use]
    pub fn builtin(&self, id: BuiltinId) -> &BuiltinData {
        &self.data.builtins[id.0 as usize]
    }

    // ---- name → id ----

    /// Resolve an engine class name to its id.
    #[must_use]
    pub fn class_by_name(&self, name: &str) -> Option<ClassId> {
        self.class_by_name.get(name).copied()
    }

    /// Resolve a builtin type name to its id.
    #[must_use]
    pub fn builtin_by_name(&self, name: &str) -> Option<BuiltinId> {
        self.builtin_by_name.get(name).copied()
    }

    /// The cached id of the `int` builtin.
    #[must_use]
    pub fn int_builtin(&self) -> Option<BuiltinId> {
        self.int_builtin
    }

    /// Resolve a singleton symbol (`Input`, `OS`, …) to the class it is an instance of.
    #[must_use]
    pub fn singleton(&self, name: &str) -> Option<ClassId> {
        self.singleton_by_name.get(name).copied()
    }

    /// Look up a `@GlobalScope` utility function (`sin`, `print`, …).
    #[must_use]
    pub fn utility(&self, name: &str) -> Option<&UtilityFn> {
        let i = *self.utility_by_name.get(name)?;
        self.data.utilities.get(i as usize)
    }

    /// Look up a global (`@GlobalScope`) enum (`Error`, `Key`, …).
    #[must_use]
    pub fn global_enum(&self, name: &str) -> Option<&EnumInfo> {
        let i = *self.global_enum_by_name.get(name)?;
        self.data.global_enums.get(i as usize)
    }

    /// Look up a hand-authored pseudo-constant (`PI`/`TAU`/`INF`/`NAN`).
    #[must_use]
    pub fn global_const(&self, name: &str) -> Option<&GlobalConst> {
        self.global_consts.iter().find(|c| c.name == name)
    }

    /// Look up a hand-authored GDScript builtin function (`preload`/`range`/`len`/…).
    #[must_use]
    pub fn gdscript_builtin(&self, name: &str) -> Option<&BuiltinFn> {
        self.gdscript_builtins.iter().find(|f| f.name == name)
    }

    // ---- member resolution ----

    /// Resolve `name` on `class`, walking the base chain; the nearest declarer wins.
    #[must_use]
    pub fn lookup_member(&self, class: ClassId, name: &str) -> Option<MemberRef<'_>> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            if let Some(m) = c.methods.iter().find(|m| m.name == name) {
                return Some(MemberRef::Method(m));
            }
            if let Some(p) = c.properties.iter().find(|p| p.name == name) {
                return Some(MemberRef::Property(p));
            }
            if let Some(s) = c.signals.iter().find(|s| s.name == name) {
                return Some(MemberRef::Signal(s));
            }
            if let Some(k) = c.constants.iter().find(|k| k.name == name) {
                return Some(MemberRef::Const(k));
            }
            if let Some(e) = c.enums.iter().find(|e| e.name == name) {
                return Some(MemberRef::Enum(e));
            }
            cur = c.base;
        }
        None
    }

    /// Whether `sub` is `sup` or transitively inherits it.
    #[must_use]
    pub fn is_subclass(&self, sub: ClassId, sup: ClassId) -> bool {
        let mut cur = Some(sub);
        while let Some(cid) = cur {
            if cid == sup {
                return true;
            }
            cur = self.class(cid).base;
        }
        false
    }

    /// Every member visible on `class` including inherited ones, deduped with the nearest
    /// declarer winning — the candidate set for `recv.<TAB>` completion (Playbook §4.3).
    #[must_use]
    pub fn members_of(&self, class: ClassId) -> Vec<MemberRef<'_>> {
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        let mut out = Vec::new();
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            for m in &c.methods {
                if seen.insert(&m.name) {
                    out.push(MemberRef::Method(m));
                }
            }
            for p in &c.properties {
                if seen.insert(&p.name) {
                    out.push(MemberRef::Property(p));
                }
            }
            for s in &c.signals {
                if seen.insert(&s.name) {
                    out.push(MemberRef::Signal(s));
                }
            }
            for k in &c.constants {
                if seen.insert(&k.name) {
                    out.push(MemberRef::Const(k));
                }
            }
            for e in &c.enums {
                if seen.insert(&e.name) {
                    out.push(MemberRef::Enum(e));
                }
            }
            cur = c.base;
        }
        out
    }

    // ---- builtins ----

    /// A field of a builtin type (`Vector2.x`).
    #[must_use]
    pub fn builtin_member(&self, builtin: BuiltinId, name: &str) -> Option<&crate::BuiltinMember> {
        self.builtin(builtin)
            .members
            .iter()
            .find(|m| m.name == name)
    }

    /// A method of a builtin type (`Array.size`).
    #[must_use]
    pub fn builtin_method(&self, builtin: BuiltinId, name: &str) -> Option<&MethodSig> {
        self.builtin(builtin)
            .methods
            .iter()
            .find(|m| m.name == name)
    }

    /// The operator overloads of a builtin type (the caller matches `op` + RHS).
    #[must_use]
    pub fn builtin_operators(&self, builtin: BuiltinId) -> &[OperatorSig] {
        &self.builtin(builtin).operators
    }
}
