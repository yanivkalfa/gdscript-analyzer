//! The Godot warning catalog + the emit-then-gate seam (Phase-6 Workstream 1).
//!
//! Severity is a *resolved* property, not a baked-in one. Inference records a [`RawWarning`]
//! (a code + range + message, **no severity**); the pure [`gate`] function resolves it against
//! the project's [`WarningSettings`] and the per-file [`SuppressionMap`] into a final
//! [`Diagnostic`] (or drops it). Because `gate` runs *downstream* of the cached `analyze_file`
//! query (in `gdscript-ide`'s `type_diagnostics`), editing a warning level never invalidates
//! inference — the salsa-cacheability invariant (Playbook §6).
//!
//! [`WarningCode`] is the single source of truth for the gateable Godot codes. The public
//! `Diagnostic.code` stays a stable `String` (via [`WarningCode::as_str`]) so the wire contract
//! is unchanged — the enum is internal to `gdscript-hir`.

use cstree::util::NodeOrToken;
use gdscript_base::{Diagnostic, DiagnosticSource, Severity, TextRange};
use gdscript_syntax::{GdNode, SyntaxKind};
use rustc_hash::FxHashMap;

/// A gateable Godot GDScript warning code (research/04 §2.2). Internal to `gdscript-hir`; the
/// public `Diagnostic.code` carries its [`as_str`](WarningCode::as_str) form, so the serialized
/// identity stays a stable string. Adding a variant is a compile error until every table below
/// (`as_str`, `default_level`, and `ALL`) covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarningCode {
    // Unassigned / unused.
    /// A typed local read before it is assigned.
    UnassignedVariable,
    /// A compound-assign (`x += …`) on a still-unassigned local.
    UnassignedVariableOpAssign,
    /// A local `var` that is never read.
    UnusedVariable,
    /// A local `const` that is never read.
    UnusedLocalConstant,
    /// A `_`-prefixed class member that is never read in-class.
    UnusedPrivateClassVariable,
    /// A parameter that is never read (excluding `_`-prefixed).
    UnusedParameter,
    /// A `signal` that is never emitted or connected in-file.
    UnusedSignal,
    // Shadowing.
    /// A local that shadows an outer local / parameter.
    ShadowedVariable,
    /// A member that shadows a base-class member.
    ShadowedVariableBaseClass,
    /// A `class_name` / member / local that shadows a global identifier.
    ShadowedGlobalIdentifier,
    // Control-flow (the two `UNREACHABLE_*` need the W2 CFG).
    /// Statements after an unconditional `return`/`break`/`continue` / an exhaustive `match`.
    UnreachableCode,
    /// A `match` arm after a wildcard/bind arm.
    UnreachablePattern,
    /// An expression statement whose value is unused and side-effect-free.
    StandaloneExpression,
    /// A ternary used as a statement.
    StandaloneTernary,
    /// A ternary whose two arms have incompatible types.
    IncompatibleTernary,
    // Type-safety.
    /// `return f()` where `f` is `Variant` into a `-> void`.
    UnsafeVoidReturn,
    /// A static method called through an instance.
    StaticCalledOnInstance,
    // Tool / static / await.
    /// A base `@tool` class without a local `@tool`.
    MissingTool,
    /// `@static_unload` on a class with no static variables.
    RedundantStaticUnload,
    /// `await` on a non-coroutine / non-signal value.
    RedundantAwait,
    // Assertions.
    /// `assert(true)` / an always-true constant condition.
    AssertAlwaysTrue,
    /// `assert(false)` / an always-false constant condition.
    AssertAlwaysFalse,
    // Numeric / enum.
    /// `int / int` (the decimal part is discarded).
    IntegerDivision,
    /// A `float` stored into an `int` slot.
    NarrowingConversion,
    /// An `int` assigned to an enum without a cast.
    IntAsEnumWithoutCast,
    /// An `int` compared to an enum in a `match`.
    IntAsEnumWithoutMatch,
    /// `var e: SomeEnum` with no initializer.
    EnumVariableWithoutDefault,
    // File / keyword.
    /// A file with no members.
    EmptyFile,
    /// A deprecated keyword (`yield`).
    DeprecatedKeyword,
    // Confusables.
    /// A mixed-script / homoglyph identifier.
    ConfusableIdentifier,
    /// A local declared after a same-name outer use.
    ConfusableLocalDeclaration,
    /// A use-before-declaration of a local shadowing a member.
    ConfusableLocalUsage,
    /// Reassigning a lambda capture.
    ConfusableCaptureReassignment,
    /// Modifying a temporary (master-only).
    ConfusableTemporaryModification,
    // Deprecated misuse.
    /// `obj.prop()` where `prop` is a property.
    PropertyUsedAsFunction,
    /// `obj.CONST()` where `CONST` is a constant.
    ConstantUsedAsFunction,
    /// `obj.method` used as a property.
    FunctionUsedAsProperty,
    // Type-strictness (default IGNORE — the opt-in group).
    /// `var x = …` without a `: T` annotation.
    UntypedDeclaration,
    /// A `:=` inferred declaration.
    InferredDeclaration,
    /// A property missing on a statically-known base.
    UnsafePropertyAccess,
    /// A method missing on a statically-known base.
    UnsafeMethodAccess,
    /// An `as T` through a `Variant`.
    UnsafeCast,
    /// An argument needing an unsafe implicit cast into the parameter type.
    UnsafeCallArgument,
    /// A non-void call result dropped.
    ReturnValueDiscarded,
    /// A `await`-able call whose result is not awaited (master-only).
    MissingAwait,
    // Hard-fail (default ERROR).
    /// A `:=` / inferred binding from a statically-`Variant` value.
    InferenceOnVariant,
    /// Overriding a native virtual with an incompatible signature.
    NativeMethodOverride,
    /// A `get_node(...)` default-value init that should be `@onready`.
    GetNodeDefaultWithoutOnready,
    /// `@onready` together with `@export` on one member.
    OnreadyWithExport,
}

/// Godot's `WarnLevel` (`gdscript_warning.h`): the resolved severity of a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarnLevel {
    /// The code is silenced.
    Ignore,
    /// The code is reported as a warning.
    Warn,
    /// The code is reported as an error.
    Error,
}

impl WarnLevel {
    /// The level for a `project.godot` `0|1|2` value (Ignore/Warn/Error), or `None` if out of range.
    #[must_use]
    pub fn from_int(n: u32) -> Option<Self> {
        match n {
            0 => Some(Self::Ignore),
            1 => Some(Self::Warn),
            2 => Some(Self::Error),
            _ => None,
        }
    }
}

/// The lowest Godot minor a code exists in. `Master` means "newer than any stable we bundle as
/// the default model" — gated against the project's declared engine version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Since {
    /// Present since Godot 4.3 (the earliest we model).
    V4_3,
    /// Only on Godot's master / a release newer than the bundled model.
    Master,
}

impl Since {
    /// The `(major, minor)` a code is first available in.
    #[must_use]
    pub fn min_version(self) -> (u32, u32) {
        match self {
            Self::V4_3 => (4, 3),
            Self::Master => bundled_version(),
        }
    }
}

impl WarningCode {
    /// Every code, for reverse lookup ([`from_setting_name`](WarningCode::from_setting_name)) and
    /// the W5 docgen. Must list every variant.
    pub const ALL: &'static [WarningCode] = &[
        Self::UnassignedVariable,
        Self::UnassignedVariableOpAssign,
        Self::UnusedVariable,
        Self::UnusedLocalConstant,
        Self::UnusedPrivateClassVariable,
        Self::UnusedParameter,
        Self::UnusedSignal,
        Self::ShadowedVariable,
        Self::ShadowedVariableBaseClass,
        Self::ShadowedGlobalIdentifier,
        Self::UnreachableCode,
        Self::UnreachablePattern,
        Self::StandaloneExpression,
        Self::StandaloneTernary,
        Self::IncompatibleTernary,
        Self::UnsafeVoidReturn,
        Self::StaticCalledOnInstance,
        Self::MissingTool,
        Self::RedundantStaticUnload,
        Self::RedundantAwait,
        Self::AssertAlwaysTrue,
        Self::AssertAlwaysFalse,
        Self::IntegerDivision,
        Self::NarrowingConversion,
        Self::IntAsEnumWithoutCast,
        Self::IntAsEnumWithoutMatch,
        Self::EnumVariableWithoutDefault,
        Self::EmptyFile,
        Self::DeprecatedKeyword,
        Self::ConfusableIdentifier,
        Self::ConfusableLocalDeclaration,
        Self::ConfusableLocalUsage,
        Self::ConfusableCaptureReassignment,
        Self::ConfusableTemporaryModification,
        Self::PropertyUsedAsFunction,
        Self::ConstantUsedAsFunction,
        Self::FunctionUsedAsProperty,
        Self::UntypedDeclaration,
        Self::InferredDeclaration,
        Self::UnsafePropertyAccess,
        Self::UnsafeMethodAccess,
        Self::UnsafeCast,
        Self::UnsafeCallArgument,
        Self::ReturnValueDiscarded,
        Self::MissingAwait,
        Self::InferenceOnVariant,
        Self::NativeMethodOverride,
        Self::GetNodeDefaultWithoutOnready,
        Self::OnreadyWithExport,
    ];

    /// The stable serialized identity — what `Diagnostic.code` carries (e.g. `INTEGER_DIVISION`).
    /// These strings are the frozen consumer-facing identifiers (Workstream 6).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnassignedVariable => "UNASSIGNED_VARIABLE",
            Self::UnassignedVariableOpAssign => "UNASSIGNED_VARIABLE_OP_ASSIGN",
            Self::UnusedVariable => "UNUSED_VARIABLE",
            Self::UnusedLocalConstant => "UNUSED_LOCAL_CONSTANT",
            Self::UnusedPrivateClassVariable => "UNUSED_PRIVATE_CLASS_VARIABLE",
            Self::UnusedParameter => "UNUSED_PARAMETER",
            Self::UnusedSignal => "UNUSED_SIGNAL",
            Self::ShadowedVariable => "SHADOWED_VARIABLE",
            Self::ShadowedVariableBaseClass => "SHADOWED_VARIABLE_BASE_CLASS",
            Self::ShadowedGlobalIdentifier => "SHADOWED_GLOBAL_IDENTIFIER",
            Self::UnreachableCode => "UNREACHABLE_CODE",
            Self::UnreachablePattern => "UNREACHABLE_PATTERN",
            Self::StandaloneExpression => "STANDALONE_EXPRESSION",
            Self::StandaloneTernary => "STANDALONE_TERNARY",
            Self::IncompatibleTernary => "INCOMPATIBLE_TERNARY",
            Self::UnsafeVoidReturn => "UNSAFE_VOID_RETURN",
            Self::StaticCalledOnInstance => "STATIC_CALLED_ON_INSTANCE",
            Self::MissingTool => "MISSING_TOOL",
            Self::RedundantStaticUnload => "REDUNDANT_STATIC_UNLOAD",
            Self::RedundantAwait => "REDUNDANT_AWAIT",
            Self::AssertAlwaysTrue => "ASSERT_ALWAYS_TRUE",
            Self::AssertAlwaysFalse => "ASSERT_ALWAYS_FALSE",
            Self::IntegerDivision => "INTEGER_DIVISION",
            Self::NarrowingConversion => "NARROWING_CONVERSION",
            Self::IntAsEnumWithoutCast => "INT_AS_ENUM_WITHOUT_CAST",
            Self::IntAsEnumWithoutMatch => "INT_AS_ENUM_WITHOUT_MATCH",
            Self::EnumVariableWithoutDefault => "ENUM_VARIABLE_WITHOUT_DEFAULT",
            Self::EmptyFile => "EMPTY_FILE",
            Self::DeprecatedKeyword => "DEPRECATED_KEYWORD",
            Self::ConfusableIdentifier => "CONFUSABLE_IDENTIFIER",
            Self::ConfusableLocalDeclaration => "CONFUSABLE_LOCAL_DECLARATION",
            Self::ConfusableLocalUsage => "CONFUSABLE_LOCAL_USAGE",
            Self::ConfusableCaptureReassignment => "CONFUSABLE_CAPTURE_REASSIGNMENT",
            Self::ConfusableTemporaryModification => "CONFUSABLE_TEMPORARY_MODIFICATION",
            Self::PropertyUsedAsFunction => "PROPERTY_USED_AS_FUNCTION",
            Self::ConstantUsedAsFunction => "CONSTANT_USED_AS_FUNCTION",
            Self::FunctionUsedAsProperty => "FUNCTION_USED_AS_PROPERTY",
            Self::UntypedDeclaration => "UNTYPED_DECLARATION",
            Self::InferredDeclaration => "INFERRED_DECLARATION",
            Self::UnsafePropertyAccess => "UNSAFE_PROPERTY_ACCESS",
            Self::UnsafeMethodAccess => "UNSAFE_METHOD_ACCESS",
            Self::UnsafeCast => "UNSAFE_CAST",
            Self::UnsafeCallArgument => "UNSAFE_CALL_ARGUMENT",
            Self::ReturnValueDiscarded => "RETURN_VALUE_DISCARDED",
            Self::MissingAwait => "MISSING_AWAIT",
            Self::InferenceOnVariant => "INFERENCE_ON_VARIANT",
            Self::NativeMethodOverride => "NATIVE_METHOD_OVERRIDE",
            Self::GetNodeDefaultWithoutOnready => "GET_NODE_DEFAULT_WITHOUT_ONREADY",
            Self::OnreadyWithExport => "ONREADY_WITH_EXPORT",
        }
    }

    /// The `project.godot` `debug/gdscript/warnings/<tail>` key tail — the lowercased [`as_str`].
    #[must_use]
    pub fn setting_name(self) -> String {
        self.as_str().to_ascii_lowercase()
    }

    /// A one-line human description — the source of truth for the generated Warning Reference
    /// (Workstream 5). Kept terse and stable; an exhaustive `match` so a new code must add one.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::UnassignedVariable => "A typed local is read before it is assigned a value.",
            Self::UnassignedVariableOpAssign => {
                "A compound assignment (`+=`, …) is applied to a still-unassigned local."
            }
            Self::UnusedVariable => "A local variable is declared but never read.",
            Self::UnusedLocalConstant => "A local constant is declared but never read.",
            Self::UnusedPrivateClassVariable => {
                "A `_`-prefixed class member is never read within the class."
            }
            Self::UnusedParameter => "A function parameter is never used (prefix it with `_`).",
            Self::UnusedSignal => "A signal is never emitted or connected in the file.",
            Self::ShadowedVariable => "A local shadows an outer local or parameter.",
            Self::ShadowedVariableBaseClass => "A member shadows a member of a base class.",
            Self::ShadowedGlobalIdentifier => {
                "A `class_name`, member, or local shadows a global identifier."
            }
            Self::UnreachableCode => {
                "A statement follows an unconditional `return`/`break`/`continue` (or an exhaustive `match`)."
            }
            Self::UnreachablePattern => {
                "A `match` pattern can never match (it follows a wildcard)."
            }
            Self::StandaloneExpression => "An expression statement has no effect.",
            Self::StandaloneTernary => {
                "A ternary conditional is used as a statement; its value is discarded."
            }
            Self::IncompatibleTernary => {
                "The two values of a ternary conditional have no common type."
            }
            Self::UnsafeVoidReturn => "A `Variant` value is returned from a `-> void` function.",
            Self::StaticCalledOnInstance => "A static method is called through an instance.",
            Self::MissingTool => "A class extends a `@tool` class but is not itself `@tool`.",
            Self::RedundantStaticUnload => {
                "`@static_unload` is used on a class with no static variables."
            }
            Self::RedundantAwait => "`await` is applied to a non-coroutine, non-signal value.",
            Self::AssertAlwaysTrue => "An `assert(...)` condition is always true.",
            Self::AssertAlwaysFalse => "An `assert(...)` condition is always false.",
            Self::IntegerDivision => "Integer division discards the fractional part.",
            Self::NarrowingConversion => "A `float` is stored into an `int`, losing precision.",
            Self::IntAsEnumWithoutCast => "An integer is assigned to an enum value without a cast.",
            Self::IntAsEnumWithoutMatch => "An integer is compared to an enum value in a `match`.",
            Self::EnumVariableWithoutDefault => {
                "An enum-typed variable has no explicit default value."
            }
            Self::EmptyFile => "The script file has no members, `class_name`, or `extends`.",
            Self::DeprecatedKeyword => "A deprecated keyword (e.g. `yield`) is used.",
            Self::ConfusableIdentifier => {
                "An identifier mixes scripts / uses confusable characters."
            }
            Self::ConfusableLocalDeclaration => "A local is declared after a same-name outer use.",
            Self::ConfusableLocalUsage => {
                "A local shadowing a member is used before its declaration."
            }
            Self::ConfusableCaptureReassignment => {
                "A captured variable is reassigned inside a lambda."
            }
            Self::ConfusableTemporaryModification => "A temporary value is modified in place.",
            Self::PropertyUsedAsFunction => "A property is called as if it were a function.",
            Self::ConstantUsedAsFunction => "A constant is called as if it were a function.",
            Self::FunctionUsedAsProperty => "A function is accessed as if it were a property.",
            Self::UntypedDeclaration => "A declaration has no type annotation.",
            Self::InferredDeclaration => "A declaration uses an inferred type (`:=`).",
            Self::UnsafePropertyAccess => {
                "A property is not present on the inferred type (but may be on a subtype)."
            }
            Self::UnsafeMethodAccess => {
                "A method is not present on the inferred type (but may be on a subtype)."
            }
            Self::UnsafeCast => "A value is cast through `Variant`, which is unsafe.",
            Self::UnsafeCallArgument => {
                "An argument needs an unsafe implicit cast into the parameter type."
            }
            Self::ReturnValueDiscarded => "A non-`void` call's return value is discarded.",
            Self::MissingAwait => "An awaitable call's result is not awaited.",
            Self::InferenceOnVariant => "A type is inferred from a statically-`Variant` value.",
            Self::NativeMethodOverride => {
                "A native virtual method is overridden with an incompatible signature."
            }
            Self::GetNodeDefaultWithoutOnready => {
                "A `get_node(...)` default initializer should be `@onready`."
            }
            Self::OnreadyWithExport => "`@onready` and `@export` are used together on one member.",
        }
    }

    /// Godot's `default_warning_levels[]` entry for this code.
    #[must_use]
    pub fn default_level(self) -> WarnLevel {
        match self {
            // The opt-in "type-strictness" group: IGNORE by default.
            Self::UntypedDeclaration
            | Self::InferredDeclaration
            | Self::UnsafePropertyAccess
            | Self::UnsafeMethodAccess
            | Self::UnsafeCast
            | Self::UnsafeCallArgument
            | Self::ReturnValueDiscarded
            | Self::MissingAwait => WarnLevel::Ignore,
            // The hard-fail group: ERROR by default.
            Self::InferenceOnVariant
            | Self::NativeMethodOverride
            | Self::GetNodeDefaultWithoutOnready
            | Self::OnreadyWithExport => WarnLevel::Error,
            // Everything else defaults to WARN.
            _ => WarnLevel::Warn,
        }
    }

    /// Whether this code is in the opt-in type-strictness group (the codes a standalone/`--strict`
    /// run promotes from IGNORE to WARN). Currently exactly the IGNORE-default set.
    #[must_use]
    pub fn is_opt_in(self) -> bool {
        self.default_level() == WarnLevel::Ignore
    }

    /// The lowest engine version this code exists in (for version-gating master-only codes).
    #[must_use]
    pub fn since(self) -> Since {
        match self {
            Self::ConfusableTemporaryModification | Self::MissingAwait => Since::Master,
            _ => Since::V4_3,
        }
    }

    /// The code whose [`setting_name`](WarningCode::setting_name) (case-insensitively) is `name`,
    /// for parsing `project.godot` keys and `@warning_ignore("name")` arguments.
    #[must_use]
    pub fn from_setting_name(name: &str) -> Option<WarningCode> {
        Self::ALL
            .iter()
            .copied()
            .find(|c| c.as_str().eq_ignore_ascii_case(name))
    }
}

/// An emitted-but-ungraded warning: the inference layer records these (no severity); [`gate`]
/// resolves each into a final [`Diagnostic`] or drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWarning {
    /// The byte range the warning applies to.
    pub range: TextRange,
    /// The code (the source of truth for severity + identity).
    pub code: WarningCode,
    /// The human-readable message.
    pub message: String,
}

/// The resolved warning configuration for a project (or the standalone analyzer default). Parsed
/// from `project.godot`'s `debug/gdscript/warnings/*`; passed to [`gate`].
// A settings/config struct — each bool is an independent Godot project setting, so the
// state-machine refactor `struct_excessive_bools` suggests would only obscure it.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarningSettings {
    /// `debug/gdscript/warnings/enable` — the master switch (default `true`).
    pub enabled: bool,
    /// `debug/gdscript/warnings/treat_warnings_as_errors` — escalate every WARN to ERROR.
    pub treat_as_errors: bool,
    /// Explicit per-code level overrides from `project.godot`.
    pub per_code: FxHashMap<WarningCode, WarnLevel>,
    /// `debug/gdscript/warnings/exclude_addons` — suppress warnings under `res://addons/**`.
    pub exclude_addons: bool,
    /// The project's declared engine `(major, minor)`, for version-gating master-only codes.
    pub engine: (u32, u32),
    /// When `true` (a standalone run / CLI `--strict`), the IGNORE-default opt-in group is
    /// promoted to WARN. A real `project.godot` clears this (its explicit settings win).
    pub strict_opt_in: bool,
}

impl WarningSettings {
    /// The standalone default (no `project.godot`): everything on, the opt-in strictness group
    /// promoted to WARN, addons not excluded. Matches the analyzer's pre-gating behavior.
    #[must_use]
    pub fn analyzer_default() -> Self {
        Self {
            enabled: true,
            treat_as_errors: false,
            per_code: FxHashMap::default(),
            exclude_addons: false,
            engine: bundled_version(),
            strict_opt_in: true,
        }
    }

    /// The engine-matching default for a project of declared version `engine`: Godot's own
    /// `default_warning_levels[]` (the opt-in group stays IGNORE), addons excluded.
    #[must_use]
    pub fn engine_default(engine: (u32, u32)) -> Self {
        Self {
            enabled: true,
            treat_as_errors: false,
            per_code: FxHashMap::default(),
            exclude_addons: true,
            engine,
            strict_opt_in: false,
        }
    }
}

/// The `@warning_ignore[_start|_restore]` suppression spans for one file. A warning is suppressed
/// when its range falls inside a span listing its code. (M0 ships the empty map; the CST walk that
/// populates it lands in W1 M2.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuppressionMap {
    spans: Vec<(TextRange, Vec<WarningCode>)>,
}

impl SuppressionMap {
    /// Whether `code` at `at` is suppressed by some span.
    #[must_use]
    pub fn is_suppressed(&self, code: WarningCode, at: TextRange) -> bool {
        self.spans.iter().any(|(span, codes)| {
            span.start <= at.start && at.end <= span.end && codes.contains(&code)
        })
    }

    /// Add a suppression span over `range` for `codes` (used by the W1 M2 CST builder + tests).
    pub fn push(&mut self, range: TextRange, codes: Vec<WarningCode>) {
        self.spans.push((range, codes));
    }
}

/// Build the per-file suppression map from the parsed CST (Workstream 1 M2): each
/// `@warning_ignore("code", …)` suppresses the listed codes over the **single following
/// statement/declaration**, and a `@warning_ignore_start("code")` … `@warning_ignore_restore("code")`
/// pair suppresses a region (EOF-terminated if unrestored). Unknown code names are skipped (the
/// unknown-name meta-diagnostic is deferred — see `TECH_DEBT.md`).
#[must_use]
pub fn build_suppression_map(root: &GdNode) -> SuppressionMap {
    let mut map = SuppressionMap::default();
    // Annotations in source order.
    let mut anns: Vec<GdNode> = gdscript_syntax::ast::descendants(root)
        .into_iter()
        .filter(|n| n.kind() == SyntaxKind::Annotation)
        .collect();
    anns.sort_by_key(|n| u32::from(n.text_range().start()));

    // Open region starts for `_start`/`_restore`, by code (most-recent-wins on restore).
    let mut open: Vec<(WarningCode, u32)> = Vec::new();
    let eof = u32::from(root.text_range().end());

    for ann in &anns {
        let Some(name) = annotation_name(ann) else {
            continue;
        };
        let codes = annotation_warning_codes(ann);
        if codes.is_empty() {
            continue; // not a `@warning_ignore*` with a recognized code
        }
        match name.as_str() {
            "warning_ignore" => {
                if let Some(target) = next_decorated_sibling(ann) {
                    let r = target.text_range();
                    map.push(
                        TextRange::new(u32::from(r.start()), u32::from(r.end())),
                        codes,
                    );
                }
            }
            "warning_ignore_start" => {
                let start = u32::from(ann.text_range().end());
                for c in codes {
                    open.push((c, start));
                }
            }
            "warning_ignore_restore" => {
                let end = u32::from(ann.text_range().start());
                for c in &codes {
                    if let Some(pos) = open.iter().rposition(|(oc, _)| oc == c) {
                        let (oc, start) = open.remove(pos);
                        map.push(TextRange::new(start, end), vec![oc]);
                    }
                }
            }
            _ => {}
        }
    }
    // Unrestored regions run to end of file.
    for (c, start) in open {
        map.push(TextRange::new(start, eof), vec![c]);
    }
    map
}

/// The annotation's name token (the identifier after `@`).
fn annotation_name(ann: &GdNode) -> Option<String> {
    ann.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == SyntaxKind::Ident)
        .map(|t| t.text().to_owned())
}

/// The recognized warning codes named by a `@warning_ignore*` annotation's string arguments.
fn annotation_warning_codes(ann: &GdNode) -> Vec<WarningCode> {
    let Some(arglist) = ann.children().find(|c| c.kind() == SyntaxKind::ArgList) else {
        return Vec::new();
    };
    let mut codes = Vec::new();
    for lit in arglist
        .children()
        .filter(|c| c.kind() == SyntaxKind::Literal)
    {
        for tok in lit
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
        {
            if tok.kind() == SyntaxKind::String
                && let Some(c) =
                    WarningCode::from_setting_name(tok.text().trim_matches(['"', '\'']))
            {
                codes.push(c);
            }
        }
    }
    codes
}

/// The single statement/declaration a `@warning_ignore` decorates — the next sibling node that is
/// not itself an annotation (annotations stack: `@onready @warning_ignore("…") var x`).
fn next_decorated_sibling(ann: &GdNode) -> Option<GdNode> {
    let parent = ann.parent()?;
    let after = ann.text_range().start();
    parent
        .children()
        .filter(|c| c.text_range().start() > after && c.kind() != SyntaxKind::Annotation)
        .min_by_key(|c| u32::from(c.text_range().start()))
        .cloned()
}

/// Resolve one [`RawWarning`] into a final [`Diagnostic`], or drop it. The **only** place
/// settings/version/suppression touch a warning — pure, so it is trivially cacheable and testable.
/// Precedence (research/04 §2.3): enable → per-code level → treat-as-errors → scope → suppression.
#[must_use]
pub fn gate(
    raw: &RawWarning,
    settings: &WarningSettings,
    ignores: &SuppressionMap,
    path: Option<&str>,
) -> Option<Diagnostic> {
    if !settings.enabled {
        return None;
    }
    // Version-gate: a code the project's engine predates never fires.
    if raw.code.since().min_version() > settings.engine {
        return None;
    }
    // Base level: an explicit override wins; else the engine default, with the opt-in group
    // promoted to WARN under `strict_opt_in`.
    let mut level = settings
        .per_code
        .get(&raw.code)
        .copied()
        .unwrap_or_else(|| {
            let d = raw.code.default_level();
            if settings.strict_opt_in && d == WarnLevel::Ignore {
                WarnLevel::Warn
            } else {
                d
            }
        });
    if level == WarnLevel::Ignore {
        return None;
    }
    if settings.treat_as_errors && level == WarnLevel::Warn {
        level = WarnLevel::Error;
    }
    if settings.exclude_addons && path.is_some_and(is_addon_path) {
        return None;
    }
    if ignores.is_suppressed(raw.code, raw.range) {
        return None;
    }
    Some(Diagnostic {
        range: raw.range,
        severity: match level {
            WarnLevel::Error => Severity::Error,
            // `Ignore` was returned above; only `Warn` reaches here besides `Error`.
            _ => Severity::Warning,
        },
        code: raw.code.as_str().to_owned(),
        message: raw.message.clone(),
        source: DiagnosticSource::Type,
        fixes: Vec::new(),
    })
}

/// Render the Markdown **Warning Reference** page from the [`WarningCode`] catalog (Workstream 5
/// docgen). The single source of truth — a test asserts the committed page matches this output, so
/// the docs can never drift from the code (regenerate with `GDSCRIPT_UPDATE_DOCS=1`).
#[must_use]
pub fn render_warning_reference() -> String {
    use std::fmt::Write as _;
    let mut codes: Vec<WarningCode> = WarningCode::ALL.to_vec();
    codes.sort_by_key(|c| c.as_str());

    let mut s = String::new();
    s.push_str("<!-- @generated by `gdscript-hir` (warnings::render_warning_reference); do not edit by hand. -->\n");
    s.push_str("<!-- Regenerate: `GDSCRIPT_UPDATE_DOCS=1 cargo test -p gdscript-hir warning_reference_doc_is_current` -->\n\n");
    s.push_str("# Warning Reference\n\n");
    s.push_str(
        "Every gateable GDScript warning the analyzer can emit, with its `project.godot` setting key, \
         engine-default level, and the earliest Godot version it applies to. Configure these under \
         `[debug]` as `gdscript/warnings/<key>` (`0` = ignore, `1` = warn, `2` = error), or suppress \
         inline with `@warning_ignore(\"<key>\")`. See [Configuration](./configuration.md).\n\n",
    );
    s.push_str("| Code | Setting key | Default | Since | Description |\n");
    s.push_str("|---|---|---|---|---|\n");
    for c in codes {
        let default = match c.default_level() {
            WarnLevel::Ignore => "Ignore",
            WarnLevel::Warn => "Warn",
            WarnLevel::Error => "Error",
        };
        let since = match c.since() {
            Since::V4_3 => "4.3",
            Since::Master => "master",
        };
        let _ = writeln!(
            s,
            "| `{}` | `{}` | {default} | {since} | {} |",
            c.as_str(),
            c.setting_name(),
            c.description(),
        );
    }
    s
}

/// Whether `path` is under a `res://addons/**` directory (the `exclude_addons` scope).
fn is_addon_path(path: &str) -> bool {
    path.starts_with("res://addons/") || path.contains("/addons/")
}

/// The bundled engine `(major, minor)` — the default project version and the `Since::Master`
/// threshold. Parsed from [`gdscript_api::godot_version`] (so it tracks the bundled model, not a
/// hardcoded literal).
#[must_use]
pub fn bundled_version() -> (u32, u32) {
    parse_major_minor(gdscript_api::godot_version()).unwrap_or((4, 5))
}

/// Parse a leading `<major>.<minor>` (ignoring any `.patch`/`-suffix`) from `s`.
fn parse_major_minor(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor: u32 = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_syntax::parse;
    use std::collections::HashSet;

    fn off(src: &str, needle: &str) -> u32 {
        u32::try_from(src.find(needle).unwrap()).unwrap()
    }

    #[test]
    fn warning_reference_doc_is_current() {
        // The committed Warning Reference is generated from the catalog — keep them in lockstep.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/src/reference/warnings.md"
        );
        let generated = render_warning_reference();
        if std::env::var("GDSCRIPT_UPDATE_DOCS").is_ok() {
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, &generated).unwrap();
            return;
        }
        let on_disk = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            on_disk, generated,
            "docs/src/reference/warnings.md is stale — regenerate with \
             `GDSCRIPT_UPDATE_DOCS=1 cargo test -p gdscript-hir warning_reference_doc_is_current`",
        );
    }

    #[test]
    fn warning_ignore_suppresses_the_next_statement() {
        let src = "func f():\n\t@warning_ignore(\"integer_division\")\n\tvar x = 5 / 2\n";
        let map = build_suppression_map(&parse(src).syntax_node());
        let at = off(src, "5 / 2");
        assert!(map.is_suppressed(WarningCode::IntegerDivision, TextRange::new(at, at + 5)));
        // A different code at the same place is not suppressed.
        assert!(!map.is_suppressed(WarningCode::NarrowingConversion, TextRange::new(at, at + 5)));
    }

    #[test]
    fn warning_ignore_start_restore_suppresses_a_region() {
        let src = "@warning_ignore_start(\"unused_variable\")\nfunc f():\n\tvar a = 1\n@warning_ignore_restore(\"unused_variable\")\nfunc g():\n\tvar b = 2\n";
        let map = build_suppression_map(&parse(src).syntax_node());
        let a = off(src, "var a");
        let b = off(src, "var b");
        assert!(map.is_suppressed(WarningCode::UnusedVariable, TextRange::new(a, a + 1)));
        // After the restore, the same code is no longer suppressed.
        assert!(!map.is_suppressed(WarningCode::UnusedVariable, TextRange::new(b, b + 1)));
    }

    fn raw(code: WarningCode) -> RawWarning {
        RawWarning {
            range: TextRange::new(10, 20),
            code,
            message: "msg".to_owned(),
        }
    }

    #[test]
    fn every_code_has_a_unique_uppercase_string_that_round_trips() {
        let mut seen = HashSet::new();
        for &c in WarningCode::ALL {
            assert!(seen.insert(c.as_str()), "duplicate as_str: {}", c.as_str());
            assert_eq!(c.as_str(), c.as_str().to_ascii_uppercase());
            assert_eq!(WarningCode::from_setting_name(&c.setting_name()), Some(c));
        }
        // The set is the catalog; a missed `ALL` entry shows up as a short count.
        assert_eq!(seen.len(), 49);
    }

    #[test]
    fn disabled_drops_everything() {
        let mut s = WarningSettings::analyzer_default();
        s.enabled = false;
        assert!(
            gate(
                &raw(WarningCode::IntegerDivision),
                &s,
                &SuppressionMap::default(),
                None
            )
            .is_none()
        );
    }

    #[test]
    fn opt_in_group_is_silent_under_engine_default_but_warns_under_strict() {
        let none = SuppressionMap::default();
        let engine = WarningSettings::engine_default((4, 5));
        assert!(gate(&raw(WarningCode::UnsafeMethodAccess), &engine, &none, None).is_none());
        let strict = WarningSettings::analyzer_default(); // strict_opt_in = true
        let d = gate(&raw(WarningCode::UnsafeMethodAccess), &strict, &none, None).unwrap();
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.code, "UNSAFE_METHOD_ACCESS");
    }

    #[test]
    fn error_default_stays_error() {
        let d = gate(
            &raw(WarningCode::InferenceOnVariant),
            &WarningSettings::analyzer_default(),
            &SuppressionMap::default(),
            None,
        )
        .unwrap();
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn treat_as_errors_escalates_warn_only() {
        let none = SuppressionMap::default();
        let mut s = WarningSettings::analyzer_default();
        s.treat_as_errors = true;
        // A WARN-default code escalates to ERROR.
        let d = gate(&raw(WarningCode::IntegerDivision), &s, &none, None).unwrap();
        assert_eq!(d.severity, Severity::Error);
        // An explicitly-Ignored code is never resurrected by treat-as-errors.
        s.per_code
            .insert(WarningCode::IntegerDivision, WarnLevel::Ignore);
        assert!(gate(&raw(WarningCode::IntegerDivision), &s, &none, None).is_none());
    }

    #[test]
    fn per_code_override_sets_level() {
        let none = SuppressionMap::default();
        let mut s = WarningSettings::engine_default((4, 5));
        s.per_code
            .insert(WarningCode::UnsafeMethodAccess, WarnLevel::Error);
        let d = gate(&raw(WarningCode::UnsafeMethodAccess), &s, &none, None).unwrap();
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn exclude_addons_suppresses_by_path() {
        let mut s = WarningSettings::analyzer_default();
        s.exclude_addons = true;
        assert!(
            gate(
                &raw(WarningCode::IntegerDivision),
                &s,
                &SuppressionMap::default(),
                Some("res://addons/x/y.gd")
            )
            .is_none()
        );
        assert!(
            gate(
                &raw(WarningCode::IntegerDivision),
                &s,
                &SuppressionMap::default(),
                Some("res://game/y.gd")
            )
            .is_some()
        );
    }

    #[test]
    fn suppression_map_drops_covered_range() {
        let mut map = SuppressionMap::default();
        map.push(TextRange::new(0, 100), vec![WarningCode::IntegerDivision]);
        assert!(
            gate(
                &raw(WarningCode::IntegerDivision),
                &WarningSettings::analyzer_default(),
                &map,
                None
            )
            .is_none()
        );
        // A different code in the same span is unaffected.
        assert!(
            gate(
                &raw(WarningCode::NarrowingConversion),
                &WarningSettings::analyzer_default(),
                &map,
                None
            )
            .is_some()
        );
    }

    #[test]
    fn master_only_codes_gate_on_engine_version() {
        let none = SuppressionMap::default();
        // ConfusableTemporaryModification is WARN-default but master-only.
        let mut old = WarningSettings::engine_default((4, 3));
        old.strict_opt_in = false;
        assert!(
            gate(
                &raw(WarningCode::ConfusableTemporaryModification),
                &old,
                &none,
                None
            )
            .is_none()
        );
        let new = WarningSettings::engine_default((4, 5));
        assert!(
            gate(
                &raw(WarningCode::ConfusableTemporaryModification),
                &new,
                &none,
                None
            )
            .is_some()
        );
    }
}
