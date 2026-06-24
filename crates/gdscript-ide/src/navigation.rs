//! Cross-file navigation (Playbook §3.M5): find-references, rename, workspace symbols,
//! goto-definition — built on [`gdscript_hir::def::classify`].
//!
//! The discipline is rust-analyzer's: **resolve, don't string-match**. A cheap word-boundary text
//! pre-filter gathers candidate occurrences; each is re-classified and kept only if it resolves to
//! the *same* [`GodotDef`]. Rename is **correct or it refuses** — never a partial edit (zero
//! false edits). These are plain read-side folds over the already-memoized salsa queries; they add
//! no tracked query and are cancellation-safe (they poll `unwind_if_revision_cancelled`).

use gdscript_base::{
    FileId, FilePosition, FileRange, NavTarget, Reference, ReferenceKind, RenameError,
    SourceChange, SymbolKind, TextEdit, TextRange,
};
use gdscript_db::{Db, parse};
use gdscript_hir::def::{self, GodotDef};
use gdscript_hir::item_tree::Member;
use gdscript_hir::queries;
use gdscript_syntax::{GdNode, SyntaxKind, ast};

// ---- find references ----------------------------------------------------------------------

/// Every reference to the symbol under `pos` (including its declaration), resolve-confirmed.
#[must_use]
pub fn find_references(db: &dyn Db, pos: FilePosition) -> Vec<Reference> {
    let Some(def) = def::classify(db, pos) else {
        return Vec::new();
    };
    // Engine/builtin symbols are out of scope (we don't index engine call sites).
    if matches!(def, GodotDef::Engine { .. }) {
        return Vec::new();
    }
    let Some(name) = def_name(db, &def) else {
        return Vec::new();
    };
    let decl = decl_location(db, &def);
    let mut out = Vec::new();
    for file_id in search_scope(db, &def) {
        db.unwind_if_revision_cancelled();
        let Some(ft) = db.file_text(file_id) else {
            continue;
        };
        let text = ft.text(db);
        for hit in word_boundary_find(text, &name) {
            let cand = FilePosition {
                file: file_id,
                offset: hit,
            };
            if def::classify(db, cand).as_ref() == Some(&def) {
                let range = TextRange::new(hit, hit + u32::try_from(name.len()).unwrap_or(0));
                let kind = if decl
                    == Some(FileRange {
                        file: file_id,
                        range,
                    }) {
                    ReferenceKind::Declaration
                } else {
                    ReferenceKind::Read
                };
                out.push(Reference {
                    file: file_id,
                    range,
                    kind,
                });
            }
        }
    }
    out.sort_by_key(|r| (r.file.0, r.range.start));
    out.dedup();
    out
}

// ---- rename -------------------------------------------------------------------------------

/// Rename the symbol under `pos` to `new_name` — a cross-file [`SourceChange`], or a
/// [`RenameError`] (correct-or-refuse: never a partial/corrupting edit).
///
/// # Errors
/// See [`RenameError`].
pub fn rename(db: &dyn Db, pos: FilePosition, new_name: &str) -> Result<SourceChange, RenameError> {
    if !is_valid_ident(new_name) {
        return Err(RenameError::InvalidIdentifier {
            new_name: new_name.to_owned(),
        });
    }
    let def = def::classify(db, pos).ok_or_else(|| RenameError::NotRenamable {
        reason: "no symbol resolves at this position".to_owned(),
    })?;
    if !def.is_renameable() {
        return Err(RenameError::NotRenamable {
            reason: "engine / builtin symbols cannot be renamed".to_owned(),
        });
    }
    refuse_if_crosses_boundary(db, &def)?;
    collision_check(db, &def, new_name)?;

    let refs = find_references(db, pos);
    if refs.is_empty() {
        return Err(RenameError::NotRenamable {
            reason: "no references found".to_owned(),
        });
    }
    // Group edits per file, deterministically.
    let mut by_file: Vec<(FileId, Vec<TextEdit>)> = Vec::new();
    for r in refs {
        let edit = TextEdit {
            range: r.range,
            new_text: new_name.to_owned(),
        };
        match by_file.iter_mut().find(|(f, _)| *f == r.file) {
            Some((_, edits)) => edits.push(edit),
            None => by_file.push((r.file, vec![edit])),
        }
    }
    let edits = by_file
        .into_iter()
        .map(|(file, mut edits)| {
            edits.sort_by_key(|e| e.range.start);
            gdscript_base::FileEdit { file, edits }
        })
        .collect();
    Ok(SourceChange { edits })
}

/// Refuse a rename whose symbol is reachable via a surface we cannot safely rewrite (the
/// correct-or-refuse contract). See the §3.4 decision table.
fn refuse_if_crosses_boundary(db: &dyn Db, def: &GodotDef) -> Result<(), RenameError> {
    match def {
        // Locals have no cross-file/scene surface; class_name globals are referenced by *path* in
        // `.tscn`/`project.godot` (and the `.godot` cache is derived) — so a `.gd`-only rewrite is
        // complete. Both proceed.
        GodotDef::Local { .. } | GodotDef::Global { .. } | GodotDef::Engine { .. } => Ok(()),
        // An autoload's name is declared in `project.godot` `[autoload]`, which this milestone does
        // not rewrite — refuse rather than leave the config stale.
        GodotDef::Autoload { .. } => Err(RenameError::CrossesUnsupportedBoundary {
            what: "autoload name is declared in project.godot (not rewritten by rename)".to_owned(),
        }),
        // A method/var/const/signal can be referenced by a *string* name (`connect("m")`,
        // `Callable(o, "m")`, a `.tscn` `[connection method="m"]`) that we cannot prove denotes
        // this symbol — and scenes are not even ingested. If a same-named string literal exists
        // anywhere in the project, refuse (we never edit a string literal).
        GodotDef::Member { owner_file, name } => {
            let string_referenceable = matches!(
                member_symbol_kind(db, *owner_file, name),
                Some(
                    SymbolKind::Function
                        | SymbolKind::Method
                        | SymbolKind::Variable
                        | SymbolKind::Constant
                        | SymbolKind::Signal
                )
            );
            if string_referenceable && project_has_string_literal(db, name) {
                Err(RenameError::CrossesUnsupportedBoundary {
                    what: format!(
                        "`{name}` may be referenced by a string (connect/Callable/scene connection)"
                    ),
                })
            } else {
                Ok(())
            }
        }
    }
}

/// Refuse if `new_name` already exists in a scope the rename would affect.
fn collision_check(db: &dyn Db, def: &GodotDef, new_name: &str) -> Result<(), RenameError> {
    let collide = |file: FileId, range: TextRange| RenameError::WouldCollide {
        at: FileRange { file, range },
        with: new_name.to_owned(),
    };
    match def {
        GodotDef::Global { .. } => {
            if let Some(root) = db.source_root()
                && let Some(decl) = queries::global_registry(db, root).resolve(new_name)
            {
                return Err(collide(decl.file_id(db), TextRange::new(0, 0)));
            }
        }
        GodotDef::Member { owner_file, .. } => {
            if let Some(ft) = db.file_text(*owner_file) {
                let tree = queries::item_tree(db, ft);
                if let Some(m) = tree.member(new_name) {
                    return Err(collide(*owner_file, member_name_range(m)));
                }
            }
        }
        GodotDef::Local {
            body_file,
            body_range,
            ..
        } => {
            // A member of the owning class, or another local in the same body, would clash.
            if let Some(ft) = db.file_text(*body_file) {
                if queries::item_tree(db, ft).member(new_name).is_some() {
                    return Err(collide(*body_file, *body_range));
                }
                let fi = queries::analyze_file(db, ft);
                if let Some(unit) = fi.unit_at(body_range.start) {
                    let text = ft.text(db);
                    for b in &unit.result.bindings {
                        let r = b.name_range;
                        if text.get(r.start as usize..r.end as usize).map(str::trim)
                            == Some(new_name)
                        {
                            return Err(collide(*body_file, r));
                        }
                    }
                }
            }
        }
        GodotDef::Autoload { .. } | GodotDef::Engine { .. } => {}
    }
    Ok(())
}

// ---- goto definition ----------------------------------------------------------------------

/// The declaration target(s) of the symbol under `pos` — the inverse of [`def::classify`].
#[must_use]
pub fn goto_definition(db: &dyn Db, pos: FilePosition) -> Vec<NavTarget> {
    match def::classify(db, pos) {
        Some(def) => nav_target_of_def(db, &def).into_iter().collect(),
        None => Vec::new(),
    }
}

// ---- workspace symbols --------------------------------------------------------------------

/// Project-wide symbols matching `query`, fuzzy-ranked (`class_name`s + every file's members).
#[must_use]
pub fn workspace_symbols(db: &dyn Db, query: &str) -> Vec<NavTarget> {
    let Some(root) = db.source_root() else {
        return Vec::new();
    };
    let mut scored: Vec<(i32, NavTarget)> = Vec::new();
    let mut push = |name: &str, target: NavTarget| {
        if let Some(score) = fuzzy_score(query, name) {
            scored.push((score, target));
        }
    };
    // class_name globals.
    for (name, file) in queries::global_registry(db, root).iter() {
        if let Some(t) = class_decl_target(db, file.file_id(db)) {
            push(name, t);
        }
    }
    // Every file's top-level members.
    for &file in root.files(db) {
        db.unwind_if_revision_cancelled();
        let fid = file.file_id(db);
        let tree = queries::item_tree(db, file);
        for m in &tree.members {
            if let Some(name) = m.name() {
                push(
                    name,
                    NavTarget {
                        file: fid,
                        full_range: member_full_range(m),
                        focus_range: member_name_range(m),
                        name: name.to_owned(),
                        kind: member_symbol_kind_of(m),
                    },
                );
            }
        }
    }
    // Rank: score desc, then name asc (deterministic). Cap.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.truncate(128);
    scored.into_iter().map(|(_, t)| t).collect()
}

// ---- helpers ------------------------------------------------------------------------------

/// The symbol's name (the text pre-filter key). For a local, sliced from its decl range.
fn def_name(db: &dyn Db, def: &GodotDef) -> Option<String> {
    match def {
        GodotDef::Local {
            body_file,
            decl_name_range,
            ..
        } => {
            let ft = db.file_text(*body_file)?;
            ft.text(db)
                .get(decl_name_range.start as usize..decl_name_range.end as usize)
                .map(str::to_owned)
        }
        other => {
            let n = other.name();
            (!n.is_empty()).then(|| n.to_owned())
        }
    }
}

/// The candidate files to search for references to `def`.
fn search_scope(db: &dyn Db, def: &GodotDef) -> Vec<FileId> {
    match def {
        // A local lives in exactly one file.
        GodotDef::Local { body_file, .. } => vec![*body_file],
        // Members / globals / autoloads can be referenced anywhere in the project; the text
        // pre-filter + re-resolve prune hard, so project-wide candidates stay correct + cheap.
        _ => db
            .source_root()
            .map(|root| root.files(db).iter().map(|f| f.file_id(db)).collect())
            .unwrap_or_default(),
    }
}

/// Occurrences of `name` at identifier word boundaries (the cheap pre-filter before re-resolve).
fn word_boundary_find(text: &str, name: &str) -> Vec<u32> {
    if name.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (i, _) in text.match_indices(name) {
        let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after = i + name.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok
            && after_ok
            && let Ok(off) = u32::try_from(i)
        {
            out.push(off);
        }
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The declaration site of `def` (file + focus range), to tag the declaration in find-refs.
fn decl_location(db: &dyn Db, def: &GodotDef) -> Option<FileRange> {
    let t = nav_target_of_def(db, def)?;
    Some(FileRange {
        file: t.file,
        range: t.focus_range,
    })
}

/// The declaration [`NavTarget`] of `def`.
fn nav_target_of_def(db: &dyn Db, def: &GodotDef) -> Option<NavTarget> {
    match def {
        GodotDef::Global { decl_file, .. } => class_decl_target(db, *decl_file),
        GodotDef::Member { owner_file, name } => {
            let ft = db.file_text(*owner_file)?;
            let tree = queries::item_tree(db, ft);
            let m = tree.member(name)?;
            Some(NavTarget {
                file: *owner_file,
                full_range: member_full_range(m),
                focus_range: member_name_range(m),
                name: name.to_string(),
                kind: member_symbol_kind_of(m),
            })
        }
        GodotDef::Local {
            body_file,
            decl_name_range,
            ..
        } => {
            let ft = db.file_text(*body_file)?;
            let name = ft
                .text(db)
                .get(decl_name_range.start as usize..decl_name_range.end as usize)?
                .to_owned();
            Some(NavTarget {
                file: *body_file,
                full_range: *decl_name_range,
                focus_range: *decl_name_range,
                name,
                kind: SymbolKind::Variable,
            })
        }
        // An autoload jumps to the head of its target script (best effort).
        GodotDef::Autoload {
            name,
            target_file: Some(f),
        } => class_decl_target(db, *f).or(Some(NavTarget {
            file: *f,
            full_range: TextRange::new(0, 0),
            focus_range: TextRange::new(0, 0),
            name: name.to_string(),
            kind: SymbolKind::Class,
        })),
        GodotDef::Autoload {
            target_file: None, ..
        }
        | GodotDef::Engine { .. } => None,
    }
}

/// The `class_name` declaration target for `file`, scanned from the parse (the item tree does not
/// retain the `class_name` token range).
fn class_decl_target(db: &dyn Db, file: FileId) -> Option<NavTarget> {
    let ft = db.file_text(file)?;
    let root = parse(db, ft).syntax_node();
    let decl = ast::descendants(&root)
        .into_iter()
        .find(|n| n.kind() == SyntaxKind::ClassNameDecl)?;
    let name_node = decl
        .children()
        .find(|c| c.kind() == SyntaxKind::Name)?
        .clone();
    // The Name node can carry leading trivia; trim to the bare identifier so the focus range and
    // display name are exact (and the declaration matches a find-refs occurrence range).
    let raw = name_node.text().to_string();
    let name = raw.trim().to_owned();
    let nr = node_range(&name_node);
    let lead = u32::try_from(raw.len() - raw.trim_start().len()).unwrap_or(0);
    let focus = TextRange::new(
        nr.start + lead,
        nr.start + lead + u32::try_from(name.len()).unwrap_or(0),
    );
    Some(NavTarget {
        file,
        full_range: node_range(&decl),
        focus_range: focus,
        name,
        kind: SymbolKind::Class,
    })
}

/// Whether any `.gd` in the project quotes `name` as a string literal (the refuse trigger for
/// method/member rename — a possible `connect`/`Callable`/scene-connection reference). A
/// conservative text scan (refuse-when-unsure): it may match an unrelated string, but never edits
/// one — erring toward refusal, never toward a partial edit.
fn project_has_string_literal(db: &dyn Db, name: &str) -> bool {
    let Some(root) = db.source_root() else {
        return false;
    };
    let dq = format!("\"{name}\"");
    let sq = format!("'{name}'");
    root.files(db).iter().any(|&file| {
        let text = file.text(db);
        text.contains(&dq) || text.contains(&sq)
    })
}

fn member_symbol_kind(db: &dyn Db, owner_file: FileId, name: &str) -> Option<SymbolKind> {
    let ft = db.file_text(owner_file)?;
    let tree = queries::item_tree(db, ft);
    tree.member(name).map(member_symbol_kind_of)
}

fn member_symbol_kind_of(m: &Member) -> SymbolKind {
    match m {
        Member::Func(_) => SymbolKind::Function,
        Member::Var(_) => SymbolKind::Variable,
        Member::Const(_) => SymbolKind::Constant,
        Member::Signal(_) => SymbolKind::Signal,
        Member::Enum(_) => SymbolKind::Enum,
        Member::Class(_) => SymbolKind::Class,
    }
}

fn member_name_range(m: &Member) -> TextRange {
    match m {
        Member::Func(f) => f.name_range,
        Member::Var(v) => v.name_range,
        Member::Const(c) => c.name_range,
        Member::Signal(s) => s.name_range,
        Member::Enum(e) => e.name_range,
        Member::Class(c) => c.name_range,
    }
}

fn member_full_range(m: &Member) -> TextRange {
    match m {
        Member::Func(f) => f.range,
        Member::Var(v) => v.range,
        Member::Const(c) => c.range,
        Member::Signal(s) => s.range,
        Member::Enum(e) => e.range,
        Member::Class(c) => c.range,
    }
}

fn node_range(n: &GdNode) -> TextRange {
    let r = n.text_range();
    TextRange::new(u32::from(r.start()), u32::from(r.end()))
}

/// Whether `name` is a single valid GDScript identifier (and not a keyword).
fn is_valid_ident(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(i, b)| {
            if i == 0 {
                b.is_ascii_alphabetic() || b == b'_'
            } else {
                is_ident_byte(b)
            }
        })
        && !is_keyword(name)
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "elif"
            | "else"
            | "for"
            | "while"
            | "match"
            | "break"
            | "continue"
            | "pass"
            | "return"
            | "class"
            | "class_name"
            | "extends"
            | "is"
            | "as"
            | "self"
            | "super"
            | "func"
            | "static"
            | "const"
            | "enum"
            | "signal"
            | "var"
            | "breakpoint"
            | "preload"
            | "await"
            | "void"
            | "and"
            | "or"
            | "not"
            | "in"
            | "true"
            | "false"
            | "null"
    )
}

/// A dependency-light fuzzy score: `None` if `query` is not an in-order subsequence of `name`
/// (case-insensitive); else a score boosting exact / prefix / contiguous matches. Empty query
/// matches everything with a neutral score.
fn fuzzy_score(query: &str, name: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let (ql, nl) = (query.to_ascii_lowercase(), name.to_ascii_lowercase());
    // Subsequence test + contiguity bonus.
    let mut score = 0i32;
    let mut ni = nl.char_indices();
    let mut last_pos: Option<usize> = None;
    for qc in ql.chars() {
        loop {
            match ni.next() {
                Some((pos, nc)) if nc == qc => {
                    if last_pos == Some(pos.wrapping_sub(1)) {
                        score += 5; // contiguous run
                    }
                    last_pos = Some(pos);
                    break;
                }
                Some(_) => {}
                None => return None, // ran out — not a subsequence
            }
        }
    }
    if nl == ql {
        score += 100;
    } else if nl.starts_with(&ql) {
        score += 40;
    }
    if name == query {
        score += 20; // exact case
    }
    // Prefer shorter names (tighter match).
    score -= i32::try_from(name.len()).unwrap_or(0);
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_base::{FilePosition, ReferenceKind, RenameError};
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    fn db_with(files: &[(u32, &str)]) -> RootDatabase {
        let mut db = RootDatabase::default();
        for (id, src) in files {
            db.set_file_text(FileId(*id), src, Durability::LOW);
            db.set_file_path(FileId(*id), &format!("res://f{id}.gd"));
        }
        db.sync_source_root();
        db
    }

    fn pos(file: u32, needle: &str, nth: usize, src: &str) -> FilePosition {
        let off = src.match_indices(needle).nth(nth).expect("needle").0;
        FilePosition {
            file: FileId(file),
            offset: u32::try_from(off).unwrap(),
        }
    }

    const WIDGET: &str = "class_name Widget\nfunc make() -> int:\n\treturn 1\n";
    const USER: &str =
        "func f():\n\tvar w: Widget\n\tvar x := Widget.new()\n\tif w is Widget:\n\t\tpass\n";

    #[test]
    fn find_refs_class_name_across_files() {
        let db = db_with(&[(0, WIDGET), (1, USER)]);
        // From the declaration: decl (f0) + annotation, `.new()`, `is` (f1) = 4.
        let refs = find_references(&db, pos(0, "Widget", 0, WIDGET));
        assert_eq!(refs.len(), 4, "{refs:?}");
        assert_eq!(refs.iter().filter(|r| r.file == FileId(0)).count(), 1);
        assert_eq!(refs.iter().filter(|r| r.file == FileId(1)).count(), 3);
        assert_eq!(
            refs.iter()
                .filter(|r| r.kind == ReferenceKind::Declaration)
                .count(),
            1,
            "exactly one declaration",
        );
    }

    #[test]
    fn find_refs_local_is_confined_to_its_body() {
        let src =
            "func a():\n\tvar i := 1\n\tvar ra := i\nfunc b():\n\tvar i := 2\n\tvar rb := i\n";
        let db = db_with(&[(0, src)]);
        // find-refs on a()'s `i` (decl, 1st "var i") → only a()'s 2 occurrences, not b()'s.
        let refs = find_references(&db, pos(0, "i", 0, src));
        // Two `i`s in a() (decl + `:= i`); none from b().
        assert_eq!(refs.len(), 2, "{refs:?}");
    }

    #[test]
    fn find_refs_distinguishes_same_named_members() {
        let a = "class_name A\nfunc update():\n\tpass\nfunc go():\n\tself.update()\n";
        let b = "class_name B\nfunc update():\n\tpass\n";
        let db = db_with(&[(0, a), (1, b)]);
        // find-refs on A.update → A's decl + A's self.update(); B.update untouched.
        let refs = find_references(&db, pos(0, "update", 0, a));
        assert!(refs.iter().all(|r| r.file == FileId(0)), "{refs:?}");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn rename_class_name_rewrites_every_file() {
        let db = db_with(&[(0, WIDGET), (1, USER)]);
        let change = rename(&db, pos(0, "Widget", 0, WIDGET), "Gadget").expect("rename ok");
        let mut total = 0;
        for fe in &change.edits {
            for e in &fe.edits {
                assert_eq!(e.new_text, "Gadget");
                total += 1;
            }
        }
        assert_eq!(total, 4, "decl + 3 uses");
    }

    #[test]
    fn rename_refuses_method_referenced_by_string() {
        let src =
            "class_name A\nfunc on_press():\n\tpass\nfunc setup():\n\tconnect(\"on_press\")\n";
        let db = db_with(&[(0, src)]);
        let err = rename(&db, pos(0, "on_press", 0, src), "on_click").unwrap_err();
        assert!(
            matches!(err, RenameError::CrossesUnsupportedBoundary { .. }),
            "{err:?}",
        );
    }

    #[test]
    fn rename_refuses_autoload() {
        let mut db = db_with(&[
            (0, "func play():\n\tpass\n"),
            (1, "func f():\n\tAudio.play()\n"),
        ]);
        db.set_project_config("[autoload]\nAudio=\"*res://f0.gd\"\n");
        let err = rename(
            &db,
            pos(1, "Audio", 0, "func f():\n\tAudio.play()\n"),
            "Sound",
        )
        .unwrap_err();
        assert!(
            matches!(err, RenameError::CrossesUnsupportedBoundary { .. }),
            "{err:?}",
        );
    }

    #[test]
    fn rename_rejects_invalid_identifier() {
        let db = db_with(&[(0, WIDGET)]);
        let err = rename(&db, pos(0, "Widget", 0, WIDGET), "2bad").unwrap_err();
        assert!(
            matches!(err, RenameError::InvalidIdentifier { .. }),
            "{err:?}"
        );
        let err2 = rename(&db, pos(0, "Widget", 0, WIDGET), "func").unwrap_err();
        assert!(
            matches!(err2, RenameError::InvalidIdentifier { .. }),
            "{err2:?}"
        );
    }

    #[test]
    fn rename_refuses_engine_symbol() {
        let src = "func f():\n\tvar n: Node\n";
        let db = db_with(&[(0, src)]);
        let err = rename(&db, pos(0, "Node", 0, src), "Knot").unwrap_err();
        assert!(matches!(err, RenameError::NotRenamable { .. }), "{err:?}");
    }

    #[test]
    fn rename_detects_collision() {
        let db = db_with(&[(0, WIDGET), (1, "class_name Gadget\n")]);
        // Renaming Widget → Gadget collides with the existing class_name Gadget.
        let err = rename(&db, pos(0, "Widget", 0, WIDGET), "Gadget").unwrap_err();
        assert!(matches!(err, RenameError::WouldCollide { .. }), "{err:?}");
    }

    #[test]
    fn workspace_symbols_finds_classes_and_members() {
        let db = db_with(&[
            (0, WIDGET),
            (1, "class_name Player\nfunc jump():\n\tpass\n"),
        ]);
        let syms = workspace_symbols(&db, "ma");
        assert!(syms.iter().any(|s| s.name == "make"), "{syms:?}");
        let widgets = workspace_symbols(&db, "Widget");
        assert!(widgets.iter().any(|s| s.name == "Widget"));
    }

    #[test]
    fn goto_definition_crosses_files() {
        let db = db_with(&[(0, WIDGET), (1, USER)]);
        // goto on `Widget.new()` in f1 → the class_name decl in f0.
        let targets = goto_definition(&db, pos(1, "Widget", 1, USER));
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file, FileId(0));
        assert_eq!(targets[0].name, "Widget");
    }
}
