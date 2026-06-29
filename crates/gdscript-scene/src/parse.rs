//! The `.tscn`/`.tres` text parser (Phase-4 M0) — a wasm-clean, never-panic, byte-offset-tracking
//! scanner that produces a [`SceneModel`].
//!
//! Strategy (per `PHASE-4-M0-PLAYBOOK.md` §4): detect a binary resource and degrade; then a
//! **two-pass** scan — pass 1 sectionizes with a Variant-aware header lexer + a lossless multiline
//! value-skipper, pass 2 builds the node tree. The parser **never errors**: every malformed/unknown
//! form becomes a [`SceneProblem`] and the model degrades to the engine's `Node`-everywhere floor.
//!
//! Only ASCII bytes are structurally significant (`[ ] { } ( ) " = ; # & @ /` + newlines); UTF-8
//! multibyte sequences in names/values are all `>= 0x80` and never collide with a delimiter, so the
//! byte scan is safe and every slice boundary lands on a char boundary (it's an ASCII delimiter).

use gdscript_base::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::model::{
    ExtId, ExtResource, NodeIdx, NodeProp, SceneConnection, SceneKind, SceneModel, SceneNode,
    SceneProblem, SubResource,
};

/// Parse `.tscn`/`.tres` text into a [`SceneModel`]. Pure, never panics, never returns `Err`.
#[must_use]
pub fn parse_scene(text: &str) -> SceneModel {
    if binary_magic(text) {
        let mut m = SceneModel::empty(SceneKind::Scene);
        m.problems.push(SceneProblem::BinaryResource);
        return m;
    }
    let mut p = Parser::new(text);
    p.run();
    p.build_tree();
    p.model
}

/// Whether the (whitespace-skipped) head is a binary resource magic (`RSRC`/`RSCC`).
fn binary_magic(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    let rest = &b[i..];
    rest.starts_with(b"RSRC") || rest.starts_with(b"RSCC")
}

/// A byte range `[start, end)` into the source.
type Span = (usize, usize);

/// The header attributes we recognize (raw value byte-ranges; interpreted at dispatch).
#[derive(Default)]
struct HeaderAttrs {
    name: Option<Span>,
    typ: Option<Span>,
    parent: Option<Span>,
    instance: Option<Span>,
    instance_placeholder: Option<Span>,
    format: Option<Span>,
    uid: Option<Span>,
    script_class: Option<Span>,
    id: Option<Span>,
    path: Option<Span>,
    signal: Option<Span>,
    from: Option<Span>,
    to: Option<Span>,
    method: Option<Span>,
}

impl HeaderAttrs {
    fn set(&mut self, key: &str, value: Span) {
        let slot = match key {
            "name" => &mut self.name,
            "type" => &mut self.typ,
            "parent" => &mut self.parent,
            "instance" => &mut self.instance,
            "instance_placeholder" => &mut self.instance_placeholder,
            "format" => &mut self.format,
            "uid" => &mut self.uid,
            "script_class" => &mut self.script_class,
            "id" => &mut self.id,
            "path" => &mut self.path,
            "signal" => &mut self.signal,
            "from" => &mut self.from,
            "to" => &mut self.to,
            "method" => &mut self.method,
            _ => return, // unknown attribute — ignored, never an error
        };
        *slot = Some(value);
    }
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    model: SceneModel,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            model: SceneModel::empty(SceneKind::Scene),
        }
    }

    // ---- low-level cursor ----

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn at_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn skip_inline_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.bump();
        }
    }

    /// Skip whitespace, newlines, and `;`-comment lines (the trivia between/around sections).
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.bump(),
                Some(b';') => self.skip_to_eol(),
                _ => break,
            }
        }
    }

    fn skip_to_eol(&mut self) {
        while !matches!(self.peek(), None | Some(b'\n')) {
            self.bump();
        }
        if self.peek() == Some(b'\n') {
            self.bump();
        }
    }

    /// Read an identifier `[A-Za-z0-9_/]+` (header tag, or a key — keys may contain `/`).
    fn read_ident(&mut self) -> Option<SmolStr> {
        let start = self.pos;
        while matches!(self.peek(), Some(b) if b.is_ascii_alphanumeric() || b == b'_' || b == b'/')
        {
            self.bump();
        }
        if self.pos == start {
            None
        } else {
            self.src.get(start..self.pos).map(SmolStr::new)
        }
    }

    // ---- value lexing (the lossless skipper) ----

    /// Consume one complete value expression (string / array / dict / constructor / bare / color /
    /// `&"…"`), returning its byte span. Never panics; stops at EOF.
    fn consume_value(&mut self) -> Span {
        self.skip_inline_ws();
        let start = self.pos;
        match self.peek() {
            Some(b'"') => self.consume_quoted(),
            Some(b'&' | b'@') => {
                self.bump();
                if self.peek() == Some(b'"') {
                    self.consume_quoted();
                } else {
                    self.consume_bare();
                }
            }
            Some(b'[' | b'{' | b'(') => self.consume_balanced(),
            Some(b'#') => self.consume_color(),
            Some(_) => {
                self.consume_bare();
                // Trailing constructor / typed-array brackets: `Vector2(…)`, `Array[T]([…])`.
                while matches!(self.peek(), Some(b'(' | b'[')) {
                    self.consume_balanced();
                }
            }
            None => {}
        }
        (start, self.pos)
    }

    /// Consume a `"…"` string: honors `\\`/`\"` escapes and **literal embedded newlines** (C12).
    fn consume_quoted(&mut self) {
        self.bump(); // opening quote
        loop {
            match self.peek() {
                None => break,
                Some(b'\\') => {
                    self.bump();
                    self.bump(); // skip the escaped byte
                }
                Some(b'"') => {
                    self.bump();
                    break;
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// Consume a `(…)`/`[…]`/`{…}` value with combined bracket depth, quote- and color-aware,
    /// across physical newlines (C2, C12).
    fn consume_balanced(&mut self) {
        let mut depth: u32 = 0;
        loop {
            match self.peek() {
                None => break,
                Some(b'"') => self.consume_quoted(),
                Some(b'#') => self.consume_color(), // a Color literal, NOT a comment (C11)
                Some(b';') => self.skip_to_eol(),
                Some(b'(' | b'[' | b'{') => {
                    depth += 1;
                    self.bump();
                }
                Some(b')' | b']' | b'}') => {
                    self.bump();
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// Consume a `#RRGGBBAA` color token (hex run after `#`).
    fn consume_color(&mut self) {
        self.bump(); // '#'
        while matches!(self.peek(), Some(b) if b.is_ascii_hexdigit()) {
            self.bump();
        }
    }

    /// Consume a bare token (ident / number / sign / `inf` / `nan` / `true` / `null`).
    fn consume_bare(&mut self) {
        while matches!(
            self.peek(),
            Some(b) if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'.')
        ) {
            self.bump();
        }
    }

    // ---- header + body ----

    /// Parse a `[tag …]` header. Returns the tag and the recognized attrs. `pos` ends just after
    /// the closing `]` (or EOF if malformed). Assumes `pos` is at `[`.
    fn read_header(&mut self) -> (Option<SmolStr>, HeaderAttrs, bool) {
        self.bump(); // '['
        self.skip_inline_ws();
        let tag = self.read_ident();
        let mut attrs = HeaderAttrs::default();
        let mut closed = false;
        loop {
            self.skip_inline_ws();
            match self.peek() {
                Some(b']') => {
                    self.bump();
                    closed = true;
                    break;
                }
                // EOF or newline before `]` — a header never wraps, so this is an unclosed bracket.
                None | Some(b'\n') => break,
                Some(_) => {
                    let Some(key) = self.read_ident() else {
                        self.bump(); // stray byte — advance to avoid looping
                        continue;
                    };
                    self.skip_inline_ws();
                    if self.peek() != Some(b'=') {
                        continue; // a bare flag (none expected); ignore
                    }
                    self.bump(); // '='
                    let value = self.consume_value();
                    attrs.set(&key, value);
                }
            }
        }
        (tag, attrs, closed)
    }

    /// Read the body property lines of the current section until the next header / EOF. When
    /// `is_node`, capture `script =` and `unique_name_in_owner =`; otherwise skip every value
    /// losslessly. Returns `(script, unique_name_in_owner)`.
    fn consume_body(&mut self, is_node: bool) -> (Option<ExtId>, bool, Vec<NodeProp>) {
        let mut script = None;
        let mut unique = false;
        let mut props = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None | Some(b'[') => break, // EOF or next section
                Some(_) => {}
            }
            let key_start = self.pos;
            let Some(key) = self.read_ident() else {
                self.skip_to_eol(); // not a key line — skip it
                continue;
            };
            let key_span = TextRange::new(to_u32(key_start), to_u32(self.pos));
            self.skip_inline_ws();
            if self.peek() != Some(b'=') {
                self.skip_to_eol();
                continue;
            }
            self.bump(); // '='
            let (vs, ve) = self.consume_value();
            if is_node {
                match key.as_str() {
                    "script" => script = self.extract_ext_id(vs, ve),
                    "unique_name_in_owner" => {
                        unique = self.src.get(vs..ve).is_some_and(|v| v.trim() == "true");
                    }
                    _ => {}
                }
                props.push(NodeProp { key, key_span });
            }
            self.skip_to_eol();
        }
        (script, unique, props)
    }

    // ---- value extraction (interpret a recorded span) ----

    /// The content of a quoted-string value (escapes resolved), or the bare token text. `None` for
    /// an empty value.
    fn extract_string(&self, span: Span) -> Option<SmolStr> {
        let raw = self.src.get(span.0..span.1)?.trim();
        if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
            Some(SmolStr::new(unescape(&raw[1..raw.len() - 1])))
        } else if raw.is_empty() {
            None
        } else {
            Some(SmolStr::new(raw))
        }
    }

    /// Parse a `format=`/numeric value to `u8` (best effort).
    fn extract_u8(&self, span: Span) -> Option<u8> {
        self.extract_string(span)?.trim().parse().ok()
    }

    /// Extract the id from an `ExtResource("id")` / `ExtResource(1)` value → the string `"id"`/`"1"`.
    /// Returns `None` for any other constructor (notably `SubResource("…")`, an *inline* script /
    /// resource that has no external path — M0 records no attachment for it; M1 types the node by
    /// its declared `type=` instead).
    fn extract_ext_id(&self, start: usize, end: usize) -> Option<ExtId> {
        let v = self.src.get(start..end)?;
        let open = v.find('(')?;
        if v.get(..open)?.trim() != "ExtResource" {
            return None;
        }
        let close = v.rfind(')')?;
        if close <= open {
            return None;
        }
        let inner = v.get(open + 1..close)?.trim().trim_matches('"').trim();
        (!inner.is_empty()).then(|| ExtId(SmolStr::new(inner)))
    }

    // ---- pass 1: sectionize ----

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            if self.at_eof() {
                break;
            }
            if self.peek() == Some(b'[') {
                self.section();
            } else {
                self.skip_to_eol(); // stray content outside a section — skip
            }
        }
    }

    fn section(&mut self) {
        let start = self.pos;
        let (tag, attrs, closed) = self.read_header();
        let header_span = TextRange::new(to_u32(start), to_u32(self.pos));
        if !closed {
            self.model
                .problems
                .push(SceneProblem::MalformedHeader { at: header_span });
            // body (if any) is consumed by the dispatch's consume_body below as a best effort
        }
        match tag.as_deref() {
            Some("gd_scene") => {
                self.model.kind = SceneKind::Scene;
                self.read_scene_header(&attrs);
                self.consume_body(false);
            }
            Some("gd_resource") => {
                self.model.kind = SceneKind::Resource;
                self.read_resource_header(&attrs);
                self.consume_body(false);
            }
            Some("ext_resource") => {
                self.add_ext_resource(&attrs, header_span);
                self.consume_body(false);
            }
            Some("sub_resource") => {
                self.add_sub_resource(&attrs, header_span);
                self.consume_body(false);
            }
            Some("node") => self.add_node(&attrs, header_span),
            Some("connection") => {
                self.add_connection(&attrs, header_span);
                self.consume_body(false); // a connection has no body, but stay robust
            }
            Some("editable" | "resource") => {
                self.consume_body(false); // recognized, structurally ignored
            }
            Some(_) => {
                self.model
                    .problems
                    .push(SceneProblem::UnknownTag { at: header_span });
                self.consume_body(false);
            }
            None => {
                self.model
                    .problems
                    .push(SceneProblem::MalformedHeader { at: header_span });
                self.consume_body(false);
            }
        }
    }

    fn read_scene_header(&mut self, a: &HeaderAttrs) {
        self.model.format = a.format.and_then(|s| self.extract_u8(s));
        self.model.uid = a.uid.and_then(|s| self.extract_string(s));
        self.model.script_class = a.script_class.and_then(|s| self.extract_string(s));
    }

    fn read_resource_header(&mut self, a: &HeaderAttrs) {
        self.model.format = a.format.and_then(|s| self.extract_u8(s));
        self.model.uid = a.uid.and_then(|s| self.extract_string(s));
        self.model.script_class = a.script_class.and_then(|s| self.extract_string(s));
        self.model.resource_type = a.typ.and_then(|s| self.extract_string(s));
    }

    fn add_ext_resource(&mut self, a: &HeaderAttrs, span: TextRange) {
        let res_type = a.typ.and_then(|s| self.extract_string(s));
        let path = a.path.and_then(|s| self.extract_string(s));
        let uid = a.uid.and_then(|s| self.extract_string(s));
        let id = a.id.and_then(|s| self.extract_string(s));
        match id {
            Some(id) => {
                if res_type.is_none() || path.is_none() {
                    self.model
                        .problems
                        .push(SceneProblem::MissingExtField { at: span });
                }
                self.model.ext_resources.insert(
                    ExtId(id),
                    ExtResource {
                        res_type: res_type.unwrap_or_default(),
                        path,
                        uid,
                        span,
                    },
                );
            }
            None => self
                .model
                .problems
                .push(SceneProblem::MissingExtField { at: span }),
        }
    }

    fn add_sub_resource(&mut self, a: &HeaderAttrs, span: TextRange) {
        let res_type = a
            .typ
            .and_then(|s| self.extract_string(s))
            .unwrap_or_default();
        if let Some(id) = a.id.and_then(|s| self.extract_string(s)) {
            self.model
                .sub_resources
                .insert(ExtId(id), SubResource { res_type, span });
        }
    }

    fn add_node(&mut self, a: &HeaderAttrs, header_span: TextRange) {
        let name = a
            .name
            .and_then(|s| self.extract_string(s))
            .unwrap_or_default();
        // The **inner** name span (quotes excluded) — a precise focus range for go-to-definition and
        // the exact rewrite target for scene-aware rename (W8).
        let name_span = a.name.map_or(header_span, |sp| self.inner_span(sp));
        let decl_type = a.typ.and_then(|s| self.extract_string(s));
        let parent_path = a.parent.and_then(|s| self.extract_string(s));
        let parent_span = a.parent.map(|sp| self.inner_span(sp));
        let instance = a.instance.and_then(|(s, e)| self.extract_ext_id(s, e));
        let instance_placeholder = a.instance_placeholder.is_some();
        let (script, unique_name_in_owner, properties) = self.consume_body(true);
        self.model.nodes.push(SceneNode {
            name,
            decl_type,
            parent_path,
            parent_span,
            parent_idx: None,
            script,
            instance,
            instance_is_inherited_root: false,
            instance_placeholder,
            unique_name_in_owner,
            header_span,
            name_span,
            properties,
        });
    }

    fn add_connection(&mut self, a: &HeaderAttrs, header_span: TextRange) {
        // A connection requires `signal`/`from`/`to`/`method`; a malformed one degrades to empty
        // fields (rename simply finds no match). The spans are the inner identifier ranges so a
        // rewrite replaces exactly the name, never the surrounding quotes.
        let value = |s: Option<Span>| s.and_then(|sp| self.extract_string(sp)).unwrap_or_default();
        let span = |s: Option<Span>| s.map_or(header_span, |sp| self.inner_span(sp));
        self.model.connections.push(SceneConnection {
            signal: value(a.signal),
            signal_span: span(a.signal),
            from: value(a.from),
            from_span: span(a.from),
            to: value(a.to),
            to_span: span(a.to),
            method: value(a.method),
            method_span: span(a.method),
            header_span,
        });
    }

    /// The **inner** byte range of an attribute value `(start, end)`: leading/trailing whitespace
    /// trimmed, and a single pair of surrounding `"` quotes excluded. (Identifier-valued attributes —
    /// node names, signal/method names, node paths — carry no escapes, so this maps 1:1 to the
    /// decoded value's bytes, which is what a rename rewrites.)
    fn inner_span(&self, (s, e): Span) -> TextRange {
        let raw = self.src.get(s..e).unwrap_or("");
        let lead = raw.len() - raw.trim_start().len();
        let trail = raw.len() - raw.trim_end().len();
        let mut lo = s + lead;
        let mut hi = e - trail;
        let trimmed = &raw[lead..raw.len() - trail];
        if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
            lo += 1;
            hi -= 1;
        }
        TextRange::new(to_u32(lo), to_u32(hi))
    }

    // ---- pass 2: build the tree ----

    fn build_tree(&mut self) {
        let n = self.model.nodes.len();
        if n == 0 {
            return;
        }
        // 1. Root(s): the parent-less nodes.
        let roots: Vec<NodeIdx> = (0..n)
            .filter(|&i| self.model.nodes[i].parent_path.is_none())
            .map(|i| NodeIdx(to_u32(i)))
            .collect();
        self.model.root = roots.first().copied();
        if roots.len() > 1 {
            self.model.problems.push(SceneProblem::MultipleRoots {
                roots: roots.clone(),
            });
        } else if roots.is_empty() {
            self.model.problems.push(SceneProblem::NoRoot);
        }
        let root = self.model.root;

        // 2. Resolve parents in file order (pre-order ⇒ ancestors already registered), building the
        //    child index, the children lists, and the full-path map.
        let mut child_index: FxHashMap<(NodeIdx, SmolStr), NodeIdx> = FxHashMap::default();
        let mut children: FxHashMap<NodeIdx, Vec<NodeIdx>> = FxHashMap::default();
        let mut full_paths: Vec<SmolStr> = vec![SmolStr::default(); n];

        for i in 0..n {
            let idx = NodeIdx(to_u32(i));
            let parent_path = self.model.nodes[i].parent_path.clone();
            let name = self.model.nodes[i].name.clone();

            // Inherited-scene root: the chosen root carrying `instance=` (`set_base_scene`). Set
            // BEFORE resolving any child paths so the into-instance check below can see it. Gated on
            // being THE root (a spurious extra parent-less node in a MultipleRoots scene is not one).
            if Some(idx) == root && self.model.nodes[i].instance.is_some() {
                self.model.nodes[i].instance_is_inherited_root = true;
            }

            let parent_idx = match parent_path.as_deref() {
                None => None,
                Some(".") => root,
                Some(p) => match walk_path(root, p, &child_index) {
                    Walk::Resolved(found) => Some(found),
                    // An absolute/`..` escape is out of the slice → silently unresolved, never a
                    // dangling parent (Playbook §5/§7 — M1 degrades it to `Node`).
                    Walk::Escaped => None,
                    Walk::Missed(deepest) => {
                        // A genuine in-scene miss. If the deepest node reached — or any ancestor up
                        // to the root — is an instance boundary, the missing tail lives in an
                        // instanced/inherited sub-scene we don't recurse into (an override line) —
                        // expected, NOT dangling (Playbook C12/C13/C20). The root being an inherited
                        // scene makes every override child's missing segment a base-scene node.
                        if !self.model.descends_from_instance(deepest) {
                            self.model.problems.push(SceneProblem::DanglingParent {
                                node: idx,
                                parent_path: SmolStr::new(p),
                            });
                        }
                        None
                    }
                },
            };
            self.model.nodes[i].parent_idx = parent_idx;

            if let Some(p) = parent_idx {
                // First sibling of a given name keeps the navigable slot (matches `unique_nodes`'
                // first-wins; Godot auto-uniquifies sibling names anyway).
                child_index.entry((p, name.clone())).or_insert(idx);
                children.entry(p).or_default().push(idx);
                let pfp = &full_paths[p.0 as usize];
                let fp = if pfp.is_empty() {
                    name
                } else {
                    SmolStr::new(format!("{pfp}/{name}"))
                };
                full_paths[i] = fp.clone();
                self.model.by_path.entry(fp).or_insert(idx);
            }
        }

        // 3. Unique-name index (first wins on collision).
        for i in 0..n {
            if self.model.nodes[i].unique_name_in_owner {
                self.model
                    .unique_nodes
                    .entry(self.model.nodes[i].name.clone())
                    .or_insert(NodeIdx(to_u32(i)));
            }
        }

        // 4. Validate `script=`/`instance=` ids against the ext-resource table.
        for i in 0..n {
            let span = self.model.nodes[i].header_span;
            let refs = [
                self.model.nodes[i].script.clone(),
                self.model.nodes[i].instance.clone(),
            ];
            for id in refs.into_iter().flatten() {
                if !self.model.ext_resources.contains_key(&id) {
                    self.model
                        .problems
                        .push(SceneProblem::UnknownExtResource { id, at: span });
                }
            }
        }

        self.model.set_indices(child_index, children);
    }
}

/// The outcome of resolving a `parent=`/node path against the in-scene tree.
enum Walk {
    /// Fully resolved to a node.
    Resolved(NodeIdx),
    /// The path escapes the scene (an absolute `/root/…` or a `..` segment). Out of the M0 slice —
    /// resolves to nothing **silently** (not a dangling parent; M1 degrades it to `Node`).
    Escaped,
    /// A genuine in-scene child miss. `deepest` is the last node reached (so the caller can tell an
    /// override-into-an-instance from a real dangling parent).
    Missed(Option<NodeIdx>),
}

/// Walk a relative name-path from `root`, segment by segment, via the incrementally-built child
/// index.
fn walk_path(
    root: Option<NodeIdx>,
    path: &str,
    child_index: &FxHashMap<(NodeIdx, SmolStr), NodeIdx>,
) -> Walk {
    if path.starts_with('/') {
        return Walk::Escaped; // absolute `/root/…` — detect before splitting (leading "" segment)
    }
    let Some(mut cur) = root else {
        return Walk::Missed(None);
    };
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Walk::Escaped; // parent escape — needs the runtime tree
        }
        match child_index.get(&(cur, SmolStr::new(seg))) {
            Some(&next) => cur = next,
            None => return Walk::Missed(Some(cur)),
        }
    }
    Walk::Resolved(cur)
}

/// Resolve the C-style escapes a `.tscn` quoted string may carry. Unknown escapes pass through
/// (the backslash is dropped, the next char kept) — a lossy-but-safe simplification for M0 (escapes
/// in node names are vanishingly rare and resolved consistently for both `name=` and `parent=`).
fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other), // \" \\ and anything else → the literal char
            None => out.push('\\'),
        }
    }
    out
}

/// `usize → u32`, saturating (a `.tscn` over 4 GiB / 4 G nodes is not a real input).
fn to_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}
