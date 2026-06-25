//! The in-memory document store + the URI↔[`FileId`] bijection (rust-analyzer's VFS pattern,
//! shrunk). Each open document holds its text (`Arc<str>`, shared cheaply with the salsa input) and
//! a [`LineIndex`] rebuilt per text version, kept paired so position conversion always uses the
//! matching text.

use std::collections::HashMap;
use std::sync::Arc;

use gdscript_base::FileId;
use lsp_types::Uri;

use crate::line_index::LineIndex;

/// One tracked document: its current text + the line index for that text.
#[derive(Debug)]
pub struct Document {
    /// The full document text.
    pub text: Arc<str>,
    /// Line-start index for `text` (position conversion).
    pub line_index: LineIndex,
    /// The client's document version (echoed back on `publishDiagnostics`).
    pub version: i32,
}

/// URI↔`FileId` interner + the document overlay. `FileId`s are assigned densely and never reused, so
/// they stay stable across a document's open/close lifetime.
#[derive(Debug, Default)]
pub struct Vfs {
    next: u32,
    ids: HashMap<String, FileId>,
    uris: Vec<Uri>,
    docs: HashMap<u32, Document>,
}

impl Vfs {
    /// The (stable) `FileId` for `uri`, interning it on first sight.
    pub fn intern(&mut self, uri: &Uri) -> FileId {
        if let Some(&id) = self.ids.get(uri.as_str()) {
            return id;
        }
        let id = FileId(self.next);
        self.next += 1;
        self.ids.insert(uri.as_str().to_owned(), id);
        self.uris.push(uri.clone());
        id
    }

    /// The URI a `FileId` was interned from.
    #[must_use]
    pub fn uri(&self, id: FileId) -> Option<&Uri> {
        self.uris.get(id.0 as usize)
    }

    /// The interned `FileId` for `uri`, if any (no interning).
    #[must_use]
    pub fn id(&self, uri: &Uri) -> Option<FileId> {
        self.ids.get(uri.as_str()).copied()
    }

    /// Insert/replace a document's text (rebuilding its [`LineIndex`]) and return its `FileId`.
    pub fn upsert(&mut self, uri: &Uri, text: String, version: i32) -> FileId {
        let id = self.intern(uri);
        let text: Arc<str> = Arc::from(text);
        let line_index = LineIndex::new(&text);
        self.docs.insert(
            id.0,
            Document {
                text,
                line_index,
                version,
            },
        );
        id
    }

    /// The tracked document for `id`, if open.
    #[must_use]
    pub fn doc(&self, id: FileId) -> Option<&Document> {
        self.docs.get(&id.0)
    }

    /// Drop a document's overlay (on `didClose`). The `FileId` mapping is retained.
    pub fn close(&mut self, id: FileId) {
        self.docs.remove(&id.0);
    }
}
