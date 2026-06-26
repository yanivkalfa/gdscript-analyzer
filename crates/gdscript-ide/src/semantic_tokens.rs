//! `semantic_tokens` (Phase-5 M2): classify each meaningful token by its **contextual** role — the
//! declarations, types, parameters, members, calls, literals, and comments a TextMate grammar can't
//! reliably distinguish — for `textDocument/semanticTokens`.
//!
//! Syntactic classification (by the token's parent node): confident and cheap. Keywords/operators
//! are skipped (the editor grammar already colors them well), and an ambiguous **bare** identifier
//! *use* (variable vs. global function vs. type) is left to the grammar — resolving those via
//! inference is a follow-up refinement (it would only *add* tokens, never change these).

use gdscript_base::{
    SemanticToken, SemanticTokenType as T, TextRange, semantic_token_modifier as m,
};
use gdscript_db::{Db, FileText, parse};
use gdscript_syntax::{GdNode, GdToken, SyntaxKind as K};

/// Classify every meaningful token in `file`, in source order.
#[must_use]
pub fn semantic_tokens(db: &dyn Db, file: FileText) -> Vec<SemanticToken> {
    let parsed = parse(db, file);
    let mut out = Vec::new();
    for element in parsed.syntax_node().descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if let Some((token_type, modifiers)) = classify(token) {
            let r = token.text_range();
            out.push(SemanticToken {
                range: TextRange::new(u32::from(r.start()), u32::from(r.end())),
                token_type,
                modifiers,
            });
        }
    }
    out
}

fn classify(token: &GdToken) -> Option<(T, u32)> {
    match token.kind() {
        K::String | K::StringName | K::NodePath => Some((T::String, 0)),
        K::Int | K::Float => Some((T::Number, 0)),
        K::LineComment | K::DocComment | K::RegionComment | K::EndRegionComment => {
            Some((T::Comment, 0))
        }
        K::Ident => classify_ident(token),
        _ => None, // keywords / operators / trivia — the grammar colors these already
    }
}

fn classify_ident(token: &GdToken) -> Option<(T, u32)> {
    let parent = token.parent(); // a token always has a parent node
    match parent.kind() {
        K::TypeRef => Some((T::Type, 0)),
        K::Annotation => Some((T::Decorator, 0)),
        K::Param | K::VarargParam => Some((T::Parameter, m::DECLARATION)),
        K::FieldExpr => Some((T::Property, 0)), // the member after `.`
        K::Name => classify_decl_name(parent),
        K::NameRef => classify_name_ref(parent),
        // A declaration whose name is a direct `Ident` child (no `Name` wrapper).
        _ => classify_decl_node(parent.kind()).map(|(ty, x)| (ty, x | m::DECLARATION)),
    }
}

/// A `Name` node holds a declaration's identifier; the declaration kind is its parent.
fn classify_decl_name(name: &GdNode) -> Option<(T, u32)> {
    let (ty, extra) = classify_decl_node(name.parent()?.kind())?;
    Some((ty, extra | m::DECLARATION))
}

/// Map a declaration node kind → the token type + extra modifiers for its name.
fn classify_decl_node(kind: K) -> Option<(T, u32)> {
    Some(match kind {
        K::FuncDecl => (T::Function, 0),
        K::VarDecl | K::VarStmt => (T::Variable, 0),
        K::ConstDecl => (T::Constant, m::READONLY),
        K::ClassNameDecl | K::InnerClassDecl => (T::Class, 0),
        K::EnumDecl => (T::Enum, 0),
        K::EnumVariant => (T::EnumMember, m::READONLY),
        K::SignalDecl => (T::Signal, 0),
        K::Param | K::VarargParam => (T::Parameter, 0),
        _ => return None,
    })
}

/// A `NameRef` (a `Name`-wrapped use): the **member** of a `FieldExpr` (`a.b` → `b`) is a property;
/// a direct **call callee** (`foo(…)`) is a function. Other bare uses are ambiguous (variable vs.
/// global vs. type) and left to the grammar — resolving them via inference is a follow-up.
fn classify_name_ref(name_ref: &GdNode) -> Option<(T, u32)> {
    let parent = name_ref.parent()?;
    match parent.kind() {
        K::CallExpr if is_first_child(parent, name_ref) => Some((T::Function, 0)),
        K::FieldExpr if !is_first_child(parent, name_ref) => Some((T::Property, 0)),
        _ => None,
    }
}

/// Whether `child` is the first child *node* of `parent` (by start offset).
fn is_first_child(parent: &GdNode, child: &GdNode) -> bool {
    parent.children().next().map(|n| n.text_range()) == Some(child.text_range())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    /// Classify `src` and return the `(text, token_type)` pairs (for readable assertions).
    fn tokens(src: &str) -> Vec<(String, T)> {
        let mut db = RootDatabase::default();
        db.set_file_text(gdscript_base::FileId(0), src, Durability::LOW);
        let ft = db.file_text(gdscript_base::FileId(0)).unwrap();
        semantic_tokens(&db, ft)
            .into_iter()
            .map(|t| {
                let slice = src[t.range.start as usize..t.range.end as usize].to_owned();
                (slice, t.token_type)
            })
            .collect()
    }

    fn has(src: &str, text: &str, ty: T) -> bool {
        tokens(src).iter().any(|(t, k)| t == text && *k == ty)
    }

    #[test]
    fn declarations_types_params_and_literals() {
        let src = "class_name Foo\nextends Node\n\nconst LIMIT := 3\n\nfunc greet(name: String) -> int:\n\t# a comment\n\tvar msg := \"hi\"\n\treturn 0\n";
        assert!(has(src, "Foo", T::Class), "class_name → Class");
        assert!(has(src, "LIMIT", T::Constant), "const name → Constant");
        assert!(has(src, "greet", T::Function), "func name → Function");
        assert!(has(src, "name", T::Parameter), "param → Parameter");
        assert!(has(src, "String", T::Type), "type annotation → Type");
        assert!(has(src, "int", T::Type), "return type → Type");
        assert!(has(src, "msg", T::Variable), "var name → Variable");
        assert!(has(src, "\"hi\"", T::String), "string literal → String");
        assert!(has(src, "3", T::Number), "int literal → Number");
        assert!(has(src, "# a comment", T::Comment), "comment → Comment");
    }

    #[test]
    fn member_access_and_call_callee() {
        let src = "func f():\n\tprint(self.health)\n";
        assert!(has(src, "print", T::Function), "call callee → Function");
        assert!(
            has(src, "health", T::Property),
            "member after `.` → Property"
        );
    }

    #[test]
    fn never_panics_on_garbage() {
        for src in [
            "",
            "func",
            "@@@",
            "var x = ",
            "\"unterminated",
            "func f(:\n",
        ] {
            let _ = tokens(src); // must not panic
        }
    }
}
