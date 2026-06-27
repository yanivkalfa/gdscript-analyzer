//! The in-memory document store + the URI↔[`FileId`] bijection (rust-analyzer's VFS pattern,
//! shrunk). It holds two layers per file:
//!
//! - a **disk** layer — background files loaded by the project scan ([`project`](crate::project)),
//!   so cross-file resolution sees the whole project, and navigation can map a result in an
//!   un-opened file to a `Location`; and
//! - an **overlay** layer — the editor's live buffer for an *open* document (its text wins over disk).
//!
//! Each file also records its `res://` path (once known) so the host gets `set_file_path` exactly
//! once. Files are interned by a **canonical key** (the path resolved through `fs::canonicalize` when
//! it exists on disk), so a `didOpen` for a file the scan already loaded reuses the same `FileId`
//! instead of double-loading it (which would manufacture a false `class_name` collision).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use gdscript_base::FileId;
use lsp_types::Uri;

use crate::line_index::LineIndex;
use crate::project;

/// One open document (the editor overlay): its text + the line index for that text + the client's
/// version (echoed back on `publishDiagnostics`).
#[derive(Debug)]
pub struct Document {
    /// The full document text.
    pub text: Arc<str>,
    /// Line-start index for `text` (position conversion).
    pub line_index: LineIndex,
    /// The client's document version.
    pub version: i32,
}

/// One background file loaded from disk by the project scan (text + its line index).
#[derive(Debug)]
pub struct DiskFile {
    /// The on-disk text.
    pub text: Arc<str>,
    /// Line-start index for `text`.
    pub line_index: LineIndex,
}

/// The text + line index of a file's *effective* content (overlay if open, else disk).
#[derive(Debug)]
pub struct Snapshot {
    /// The effective text.
    pub text: Arc<str>,
    /// Its line index.
    pub line_index: LineIndex,
}

/// URI↔`FileId` interner + the document overlay + the disk layer. `FileId`s are assigned densely and
/// never reused, so they stay stable across a document's open/close lifetime.
#[derive(Debug, Default)]
pub struct Vfs {
    next: u32,
    /// Canonical-key → `FileId` (the interner; key resolves the path so open==disk).
    ids: HashMap<String, FileId>,
    /// `FileId` index → the URI it was interned from (dense, for output `Location`s).
    uris: Vec<Uri>,
    /// Open editor overlays by `FileId`.
    docs: HashMap<u32, Document>,
    /// Background on-disk files by `FileId` (the project scan).
    disk: HashMap<u32, DiskFile>,
    /// The `res://` path recorded for a `FileId`, once known.
    res: HashMap<u32, String>,
}

impl Vfs {
    /// The (stable) `FileId` for `uri`, interning it on first sight. The interner key is the
    /// canonicalized path (so the same on-disk file reached via differently-formatted URIs — drive
    /// case, percent-encoding — maps to one `FileId`).
    pub fn intern(&mut self, uri: &Uri) -> FileId {
        let key = canonical_key(uri);
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = FileId(self.next);
        self.next += 1;
        self.ids.insert(key, id);
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
        self.ids.get(&canonical_key(uri)).copied()
    }

    /// Insert/replace an **overlay** (an open document), rebuilding its [`LineIndex`]; returns its
    /// `FileId`.
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

    /// Record a background **disk** file (the project scan), interning its URI; returns its `FileId`.
    pub fn set_disk(&mut self, uri: &Uri, text: Arc<str>) -> FileId {
        let id = self.intern(uri);
        let line_index = LineIndex::new(&text);
        self.disk.insert(id.0, DiskFile { text, line_index });
        id
    }

    /// Record a file's `res://` path (idempotent; the first non-empty value sticks via the caller's
    /// `with_path` guard).
    pub fn set_res_path(&mut self, id: FileId, res_path: String) {
        self.res.insert(id.0, res_path);
    }

    /// The `res://` path recorded for `id`, if any.
    #[must_use]
    pub fn res_path(&self, id: FileId) -> Option<&str> {
        self.res.get(&id.0).map(String::as_str)
    }

    /// The tracked **overlay** document for `id`, if open.
    #[must_use]
    pub fn doc(&self, id: FileId) -> Option<&Document> {
        self.docs.get(&id.0)
    }

    /// Whether `id` has a background disk layer (i.e. it is part of the scanned project).
    #[must_use]
    pub fn has_disk(&self, id: FileId) -> bool {
        self.disk.contains_key(&id.0)
    }

    /// The *effective* content of `id`: the overlay if open, else the disk layer, else `None`.
    #[must_use]
    pub fn snapshot(&self, id: FileId) -> Option<Snapshot> {
        if let Some(d) = self.docs.get(&id.0) {
            return Some(Snapshot {
                text: Arc::clone(&d.text),
                line_index: d.line_index.clone(),
            });
        }
        let f = self.disk.get(&id.0)?;
        Some(Snapshot {
            text: Arc::clone(&f.text),
            line_index: f.line_index.clone(),
        })
    }

    /// Drop a document's **overlay** (on `didClose`); the disk layer + `FileId` mapping are retained.
    pub fn close(&mut self, id: FileId) {
        self.docs.remove(&id.0);
    }

    /// Every known `FileId` (overlay or disk), ascending — for a navigation snapshot spanning the
    /// whole project, not just open documents.
    #[must_use]
    pub fn known_ids(&self) -> Vec<FileId> {
        let mut s: BTreeSet<u32> = self.docs.keys().copied().collect();
        s.extend(self.disk.keys().copied());
        s.into_iter().map(FileId).collect()
    }
}

/// The interner key for `uri`: for a `file:` URI that exists on disk, the path resolved through
/// `fs::canonicalize` and normalized (so two spellings of one real file collide); otherwise the
/// best-effort normalized path; otherwise the raw URI string (non-`file:` schemes).
fn canonical_key(uri: &Uri) -> String {
    if let Some(path) = project::uri_to_path(uri) {
        let canon = std::fs::canonicalize(&path).unwrap_or(path);
        let mut s = canon.to_string_lossy().replace('\\', "/");
        if s.starts_with("//?/") {
            s.drain(.."//?/".len()); // strip the Windows `\\?\` verbatim prefix
        }
        if cfg!(windows) {
            s = s.to_lowercase(); // Windows paths are case-insensitive
        }
        return s;
    }
    uri.as_str().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn overlay_wins_over_disk_for_the_same_file() {
        let mut vfs = Vfs::default();
        let u = uri("file:///main.gd");
        let id = vfs.set_disk(&u, Arc::from("disk text\n"));
        assert_eq!(&*vfs.snapshot(id).unwrap().text, "disk text\n");
        vfs.upsert(&u, "overlay text\n".to_owned(), 1);
        assert_eq!(
            &*vfs.snapshot(id).unwrap().text,
            "overlay text\n",
            "an open overlay must shadow the disk layer"
        );
        vfs.close(id);
        assert_eq!(
            &*vfs.snapshot(id).unwrap().text,
            "disk text\n",
            "closing reverts to the disk layer"
        );
    }

    #[test]
    fn disk_then_open_reuses_one_file_id() {
        // The collision-avoidance guarantee: a didOpen for a scanned file is the SAME FileId.
        let mut vfs = Vfs::default();
        let u = uri("file:///x.gd");
        let disk_id = vfs.set_disk(&u, Arc::from("a\n"));
        let open_id = vfs.upsert(&u, "a\n".to_owned(), 1);
        assert_eq!(disk_id, open_id, "open must reuse the scanned FileId");
    }

    #[test]
    fn known_ids_spans_disk_and_overlay() {
        let mut vfs = Vfs::default();
        let a = vfs.set_disk(&uri("file:///a.gd"), Arc::from("a\n"));
        let b = vfs.upsert(&uri("file:///b.gd"), "b\n".to_owned(), 1);
        let ids = vfs.known_ids();
        assert!(ids.contains(&a) && ids.contains(&b));
    }

    #[test]
    fn res_path_is_recorded_and_read_back() {
        let mut vfs = Vfs::default();
        let id = vfs.set_disk(&uri("file:///a.gd"), Arc::from("a\n"));
        assert_eq!(vfs.res_path(id), None);
        vfs.set_res_path(id, "res://a.gd".to_owned());
        assert_eq!(vfs.res_path(id), Some("res://a.gd"));
    }
}
