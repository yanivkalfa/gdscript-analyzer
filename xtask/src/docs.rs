//! Godot doc-XML → per-symbol Markdown hover docs (GODOT-SYNC.md §4, Playbook §4.6).
//!
//! Reads the vendored `doc/classes/*.xml` (engine classes, builtins, the `@GlobalScope` /
//! `@GDScript` pseudo-classes), extracts each symbol's `<description>` prose, converts Godot
//! `BBCode` to **Markdown**, and exposes the result as a [`DocSet`]. The codegen ([`crate::codegen`])
//! then interns the per-symbol Markdown into a [`gdscript_api::DocStore`] and tags each model
//! symbol with its [`gdscript_api::DocId`].
//!
//! This is codegen-time only — the runtime serves pre-converted Markdown strings, so neither the
//! XML parser nor the `BBCode` converter ever reaches `wasm`/the analyzer hot path.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use gdscript_api::{ApiData, DocId, DocStore};

/// The Godot online class-reference base, substituted for `$DOCS_URL` in doc prose.
const DOCS_URL: &str = "https://docs.godotengine.org/en/4.5";

/// One class/builtin/pseudo-class worth of doc text, keyed by symbol. Descriptions are stored as
/// **raw `BBCode`** and converted to Markdown lazily (so identical conversions dedup at intern time).
#[derive(Default)]
struct ClassDoc {
    /// The `<brief_description>` (raw `BBCode`).
    brief: String,
    /// The `<description>` (raw `BBCode`).
    desc: String,
    /// Member descriptions, keyed by `(section, name)` — `section ∈ {method, member, signal,
    /// constant}` matching the doc-XML child tag.
    members: HashMap<(&'static str, String), String>,
}

/// The parsed doc corpus: class/builtin name → its [`ClassDoc`]. `@GlobalScope` carries the
/// utility-function docs; `@GDScript` the GDScript-layer builtins/annotations.
#[derive(Default)]
pub struct DocSet {
    classes: HashMap<String, ClassDoc>,
}

/// Interns converted Markdown into a [`DocStore`], deduplicating identical text. An empty doc is
/// never interned — the symbol keeps `doc: None`.
#[derive(Default)]
struct DocBuilder {
    entries: Vec<String>,
    dedup: HashMap<String, DocId>,
}

impl DocBuilder {
    /// Intern Markdown text, returning its handle. `None`/empty text interns nothing.
    fn intern(&mut self, md: Option<String>) -> Option<DocId> {
        let md = md?;
        if md.trim().is_empty() {
            return None;
        }
        if let Some(id) = self.dedup.get(&md) {
            return Some(*id);
        }
        let id = DocId(u32::try_from(self.entries.len()).unwrap_or(u32::MAX));
        self.entries.push(md.clone());
        self.dedup.insert(md, id);
        Some(id)
    }

    fn finish(self) -> DocStore {
        DocStore {
            entries: self.entries,
        }
    }
}

impl DocSet {
    /// Parse every `*.xml` under `dir` into a doc corpus. Unreadable / malformed files are skipped
    /// (logged to stderr) rather than failing the whole codegen — a single bad vendored file should
    /// not block a regen.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut set = DocSet::default();
        let entries =
            std::fs::read_dir(dir).with_context(|| format!("reading doc dir {}", dir.display()))?;
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xml") {
                continue;
            }
            let xml = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("docs: skipping {} ({e})", path.display());
                    continue;
                }
            };
            if let Err(e) = set.ingest(&xml) {
                eprintln!("docs: skipping {} ({e})", path.display());
            }
        }
        Ok(set)
    }

    /// Parse one `<class>` doc document into the corpus.
    fn ingest(&mut self, xml: &str) -> Result<()> {
        let doc = roxmltree::Document::parse(xml).context("parsing doc XML")?;
        let class = doc.root_element();
        if !class.has_tag_name("class") {
            return Ok(());
        }
        let Some(name) = class.attribute("name") else {
            return Ok(());
        };
        let mut cd = ClassDoc::default();
        for child in class.children().filter(roxmltree::Node::is_element) {
            match child.tag_name().name() {
                "brief_description" => cd.brief = node_text(child),
                "description" => cd.desc = node_text(child),
                "methods" => Self::ingest_members(&mut cd, child, "method"),
                "members" => Self::ingest_members(&mut cd, child, "member"),
                "signals" => Self::ingest_members(&mut cd, child, "signal"),
                "constants" => Self::ingest_members(&mut cd, child, "constant"),
                _ => {}
            }
        }
        self.classes.insert(name.to_owned(), cd);
        Ok(())
    }

    /// Ingest a `<methods>`/`<members>`/`<signals>`/`<constants>` section. Each entry's description
    /// is either a `<description>` child (methods/signals) or the element's own text
    /// (members/constants).
    fn ingest_members(cd: &mut ClassDoc, section: roxmltree::Node, kind: &'static str) {
        for item in section.children().filter(roxmltree::Node::is_element) {
            let Some(name) = item.attribute("name") else {
                continue;
            };
            if let Some(text) = member_desc(item) {
                cd.members.insert((kind, name.to_owned()), text);
            }
        }
    }

    /// The combined class-level Markdown (brief + full description), or `None` if both are empty.
    fn class_md(cd: &ClassDoc) -> Option<String> {
        let brief = non_empty(bbcode_to_md(&dedent(&cd.brief)));
        let desc = non_empty(bbcode_to_md(&dedent(&cd.desc)));
        match (brief, desc) {
            (Some(b), Some(d)) if b == d => Some(b),
            (Some(b), Some(d)) => Some(format!("{b}\n\n{d}")),
            (Some(b), None) => Some(b),
            (None, Some(d)) => Some(d),
            (None, None) => None,
        }
    }

    /// The Markdown for a member, by `(class, section, name)`.
    fn member_md(&self, class: &str, kind: &'static str, name: &str) -> Option<String> {
        let raw = self
            .classes
            .get(class)?
            .members
            .get(&(kind, name.to_owned()))?;
        non_empty(bbcode_to_md(&dedent(raw)))
    }
}

/// Tag every symbol in `api` with a [`DocId`] from `docs`, returning the populated [`DocStore`].
///
/// A second pass over the already-normalized model (rather than threading a doc builder through
/// every `normalize_*` helper): each class/builtin/utility looks up its prose, converts it, and
/// interns it. Symbols with no doc keep `doc: None`.
pub fn attach_docs(api: &mut ApiData, docs: &DocSet) -> DocStore {
    let mut b = DocBuilder::default();

    for class in &mut api.classes {
        let cd = docs.classes.get(&class.name);
        class.doc = b.intern(cd.and_then(DocSet::class_md));
        let name = class.name.clone();
        for m in &mut class.methods {
            m.doc = b.intern(docs.member_md(&name, "method", &m.name));
        }
        for p in &mut class.properties {
            p.doc = b.intern(docs.member_md(&name, "member", &p.name));
        }
        for s in &mut class.signals {
            s.doc = b.intern(docs.member_md(&name, "signal", &s.name));
        }
        for k in &mut class.constants {
            k.doc = b.intern(docs.member_md(&name, "constant", &k.name));
        }
    }

    for bt in &mut api.builtins {
        let cd = docs.classes.get(&bt.name);
        bt.doc = b.intern(cd.and_then(DocSet::class_md));
        let name = bt.name.clone();
        for m in &mut bt.methods {
            m.doc = b.intern(docs.member_md(&name, "method", &m.name));
        }
        for k in &mut bt.constants {
            k.doc = b.intern(docs.member_md(&name, "constant", &k.name));
        }
    }

    // Utility functions are documented as `<method>`s of the `@GlobalScope` pseudo-class.
    for u in &mut api.utilities {
        u.doc = b.intern(docs.member_md("@GlobalScope", "method", &u.name));
    }

    b.finish()
}

/// A member's description: a `<description>` child (methods/signals) or the element's own direct
/// text (members/constants). `None` when empty.
fn member_desc(node: roxmltree::Node) -> Option<String> {
    if let Some(d) = node
        .children()
        .find(|c| c.is_element() && c.has_tag_name("description"))
    {
        return non_empty(node_text(d));
    }
    non_empty(node_text(node))
}

/// Concatenate a node's **direct** text children (`BBCode` is plain text, XML-unescaped by the
/// parser). Element children (e.g. `<param>` in a `<method>`) contribute nothing.
fn node_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|c| c.text())
        .collect()
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

/// Strip the common leading whitespace shared by every non-blank line (the XML nesting indent),
/// preserving the *relative* indentation a `[codeblock]` relies on, then trim leading/trailing
/// blank lines. Godot doc prose is authored tab-indented to match XML depth.
fn dedent(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<&str> = lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l.trim_start()
            }
        })
        .collect();
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Convert Godot `BBCode` to Markdown (GODOT-SYNC.md §4). Unhandled tags are dropped, so no literal
/// `[…]` leaks into hover. `$DOCS_URL` is substituted first.
fn bbcode_to_md(src: &str) -> String {
    let src = src.replace("$DOCS_URL", DOCS_URL);
    let mut out = String::with_capacity(src.len());
    let mut rest = src.as_str();

    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..]; // starts at '['
        let Some(close_rel) = tail.find(']') else {
            // Unbalanced '[': emit it literally and move on.
            out.push('[');
            rest = &tail[1..];
            continue;
        };
        let tag = &tail[1..close_rel];
        let after = &tail[close_rel + 1..];
        rest = apply_tag(tag, after, &mut out);
    }
    out.push_str(rest);
    out
}

/// Handle one `[tag]` whose closing `]` has already been consumed; `after` is the text following
/// it. Returns the unconsumed remainder (content-pair tags consume through their close tag).
fn apply_tag<'a>(tag: &str, after: &'a str, out: &mut String) -> &'a str {
    match tag {
        // Inline emphasis — open and close map to the same Markdown marker.
        "b" | "/b" => out.push_str("**"),
        "i" | "/i" => out.push('*'),
        "code" => return inline_code(after, out),
        "kbd" => return wrap_pair(after, "[/kbd]", "`", "`", out),
        // Structural / styling tags we strip but keep the inner text of.
        "u" | "/u" | "s" | "/s" | "center" | "/center" | "/color" | "/font" | "/url"
        | "codeblocks" | "/codeblocks" => {}
        // Hard line break / escaped brackets.
        "br" => out.push('\n'),
        "lb" => out.push('['),
        "rb" => out.push(']'),
        // Code blocks (verbatim, dedented). `[csharp]` is dropped — hover shows GDScript only.
        "codeblock" | "gdscript" => return code_block(after, &format!("[/{tag}]"), out),
        "csharp" => return drop_pair(after, "[/csharp]"),
        _ => return apply_open_or_ref(tag, after, out),
    }
    after
}

/// Tags that need their *content* (a `[color=…]`, `[font=…]`, `[url=href]`, or a cross-reference).
fn apply_open_or_ref<'a>(tag: &str, after: &'a str, out: &mut String) -> &'a str {
    // `[codeblock lang=text]` — a non-GDScript fenced block.
    if let Some(rest) = tag.strip_prefix("codeblock ") {
        let lang = rest.strip_prefix("lang=").unwrap_or("").trim();
        return code_block_lang(after, "[/codeblock]", lang, out);
    }
    // Styling wrappers we open and later close (the `/color`,`/font` close is handled above).
    if tag.starts_with("color=") || tag.starts_with("font=") || tag == "color" || tag == "font" {
        return after;
    }
    // `[url=href]text[/url]` → `[text](href)`.
    if let Some(href) = tag.strip_prefix("url=") {
        if let Some(end) = after.find("[/url]") {
            out.push('[');
            out.push_str(after[..end].trim());
            out.push_str("](");
            out.push_str(href);
            out.push(')');
            return &after[end + "[/url]".len()..];
        }
        out.push_str(href);
        return after;
    }
    // `[url]href[/url]` → the bare URL.
    if tag == "url" {
        if let Some(end) = after.find("[/url]") {
            out.push_str(after[..end].trim());
            return &after[end + "[/url]".len()..];
        }
        return after;
    }
    // A cross-reference (`[method X]`, `[Node]`, `[constant Y]`, …) → a backticked symbol.
    if let Some(md) = xref(tag) {
        out.push_str(&md);
    }
    // Otherwise: an unknown/unhandled tag — drop it (no literal `[…]` leaks).
    after
}

/// `[code]…[/code]` — inline code, verbatim (`BBCode` inside is not interpreted).
fn inline_code<'a>(after: &'a str, out: &mut String) -> &'a str {
    wrap_pair(after, "[/code]", "`", "`", out)
}

/// Emit `open + verbatim_content + close`, consuming through `close_tag`. Falls back to just `open`
/// when the close is missing.
fn wrap_pair<'a>(
    after: &'a str,
    close_tag: &str,
    open: &str,
    close: &str,
    out: &mut String,
) -> &'a str {
    if let Some(end) = after.find(close_tag) {
        out.push_str(open);
        out.push_str(&after[..end]);
        out.push_str(close);
        return &after[end + close_tag.len()..];
    }
    out.push_str(open);
    after
}

/// Drop a tag pair and its content entirely (used for `[csharp]`).
fn drop_pair<'a>(after: &'a str, close_tag: &str) -> &'a str {
    after
        .find(close_tag)
        .map_or(after, |end| &after[end + close_tag.len()..])
}

/// A GDScript fenced code block.
fn code_block<'a>(after: &'a str, close_tag: &str, out: &mut String) -> &'a str {
    code_block_lang(after, close_tag, "gdscript", out)
}

/// A fenced code block in `lang` (empty `lang` → a plain fence).
fn code_block_lang<'a>(after: &'a str, close_tag: &str, lang: &str, out: &mut String) -> &'a str {
    let (content, rest) = match after.find(close_tag) {
        Some(end) => (&after[..end], &after[end + close_tag.len()..]),
        None => (after, ""),
    };
    out.push_str("\n```");
    out.push_str(lang);
    out.push('\n');
    out.push_str(dedent(content).trim_end());
    out.push_str("\n```\n");
    rest
}

/// Convert a cross-reference tag to a backticked symbol. `None` for a tag that isn't a reference
/// (so the caller drops it).
fn xref(tag: &str) -> Option<String> {
    // `[method Class.foo]` / `[method foo]` → `Class.foo()` / `foo()`.
    for kw in ["method ", "constructor ", "operator "] {
        if let Some(sym) = tag.strip_prefix(kw) {
            return Some(format!("`{}()`", sym.trim()));
        }
    }
    for kw in [
        "member ",
        "constant ",
        "signal ",
        "enum ",
        "param ",
        "theme_item ",
        "annotation ",
        "bitfield ",
    ] {
        if let Some(sym) = tag.strip_prefix(kw) {
            return Some(format!("`{}`", sym.trim()));
        }
    }
    // A bare symbol reference: `[Node]`, `[Vector2]`, `[@GlobalScope]`, `[Object.free]`.
    if is_symbol_ref(tag) {
        return Some(format!("`{tag}`"));
    }
    None
}

/// Whether `tag` is a bare class/identifier reference (an optional leading `@`, then
/// identifier characters and dots) — not an unbalanced or styling tag.
fn is_symbol_ref(tag: &str) -> bool {
    let body = tag.strip_prefix('@').unwrap_or(tag);
    !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && body.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_and_inline_code() {
        assert_eq!(
            bbcode_to_md("[b]bold[/b] and [i]it[/i]"),
            "**bold** and *it*"
        );
        assert_eq!(bbcode_to_md("call [code]foo()[/code]"), "call `foo()`");
    }

    #[test]
    fn cross_references_become_backticked_symbols() {
        assert_eq!(bbcode_to_md("see [method add_child]"), "see `add_child()`");
        assert_eq!(
            bbcode_to_md("see [method Object.free]"),
            "see `Object.free()`"
        );
        assert_eq!(
            bbcode_to_md("a [Node] and [member owner]"),
            "a `Node` and `owner`"
        );
        assert_eq!(
            bbcode_to_md("[constant NOTIFICATION_READY]"),
            "`NOTIFICATION_READY`"
        );
        assert_eq!(bbcode_to_md("the [param delta]"), "the `delta`");
    }

    #[test]
    fn inline_code_is_verbatim_not_reinterpreted() {
        // A `[i]` *inside* inline code must stay literal, not become emphasis.
        assert_eq!(bbcode_to_md("[code]arr[i][/code]"), "`arr[i]`");
    }

    #[test]
    fn urls_and_docs_url() {
        assert_eq!(
            bbcode_to_md("[url=https://x.com]site[/url]"),
            "[site](https://x.com)"
        );
        assert_eq!(
            bbcode_to_md("see $DOCS_URL/tutorials/x.html"),
            "see https://docs.godotengine.org/en/4.5/tutorials/x.html"
        );
    }

    #[test]
    fn codeblock_is_fenced_gdscript() {
        let md = bbcode_to_md("[codeblock]\nvar x = 1\n[/codeblock]");
        assert!(md.contains("```gdscript"));
        assert!(md.contains("var x = 1"));
    }

    #[test]
    fn codeblocks_wrapper_keeps_gdscript_drops_csharp() {
        let md = bbcode_to_md(
            "[codeblocks][gdscript]var x = 1[/gdscript][csharp]int x = 1;[/csharp][/codeblocks]",
        );
        assert!(md.contains("var x = 1"));
        assert!(!md.contains("int x = 1;"), "C# block must be dropped: {md}");
    }

    #[test]
    fn styling_tags_stripped_brackets_escaped_classes_backticked() {
        // A bare identifier is a class reference → backticked (Godot's `[ClassName]` link form).
        assert_eq!(bbcode_to_md("a [SomeClass] b"), "a `SomeClass` b");
        // Styling wrappers are stripped, inner text kept.
        assert_eq!(bbcode_to_md("[color=red]warn[/color]"), "warn");
        assert_eq!(bbcode_to_md("[font=x]text[/font]"), "text");
        // A genuinely-unknown *structured* tag (with `=`/spaces) is dropped — no literal leak.
        assert_eq!(bbcode_to_md("a [weird tag=1] b"), "a  b");
        // `[lb]`/`[rb]` are the escaped literal brackets.
        assert_eq!(bbcode_to_md("an array [lb]0[rb]"), "an array [0]");
    }

    #[test]
    fn dedent_preserves_relative_codeblock_indent() {
        let s = "\t\tHello.\n\t\t[codeblock]\n\t\t\tindented\n\t\t[/codeblock]";
        let out = dedent(s);
        assert!(out.starts_with("Hello."));
        // The codeblock's inner line keeps one tab of relative indent.
        assert!(out.contains("\n\tindented"));
    }

    #[test]
    fn intern_dedups_and_skips_empty() {
        let mut b = DocBuilder::default();
        assert_eq!(b.intern(None), None);
        assert_eq!(b.intern(Some("   ".to_owned())), None);
        let a = b.intern(Some("doc".to_owned())).unwrap();
        let c = b.intern(Some("doc".to_owned())).unwrap();
        assert_eq!(a, c, "identical text dedups to one entry");
        assert_eq!(b.finish().entries.len(), 1);
    }
}
