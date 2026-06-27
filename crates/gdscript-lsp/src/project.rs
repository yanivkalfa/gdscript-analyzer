//! Workspace discovery for the standalone server.
//!
//! The standalone LSP, unlike an editor extension, gets no project model handed to it — so on
//! `initialize` it walks the client's workspace roots to the nearest `project.godot`, then loads
//! **every** `.gd` and `.tscn` under that root into the one [`AnalysisHost`](gdscript_ide::AnalysisHost),
//! feeding each file's `res://` path and the `project.godot` text. That is what lights up cross-file
//! resolution — `class_name` globals, autoloads, `preload`/`extends`, and scene-typed node paths —
//! which the per-open-file model could never do. Mirrors the CLI's `engine.rs` loader, but yields LSP
//! [`Uri`]s (the server is URI-keyed) and includes `.tscn` scenes (the CLI lints only `.gd`).
//!
//! The filesystem is touched **here only** (and in the [`vfs`](crate::vfs) canonicalizer); the
//! `gdscript-ide` core never reads a file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use lsp_types::Uri;

/// One discovered project file, ready to feed the host.
#[derive(Debug)]
pub struct LoadedFile {
    /// The `file://` URI it was discovered at (used for output `Location`s).
    pub uri: Uri,
    /// Its on-disk text (UTF-8, lossily decoded so a stray byte never aborts the scan).
    pub text: Arc<str>,
    /// Its `res://` path under the project root, if it resolved (feeds cross-file resolution).
    pub res_path: Option<String>,
}

/// The result of scanning a workspace: every `.gd`/`.tscn` file + the `project.godot` text (if any).
#[derive(Debug, Default)]
pub struct LoadedProject {
    /// The project root (the `project.godot` directory) — kept so later file-watch events can compute
    /// a `res://` path for a newly-created file.
    pub root: Option<PathBuf>,
    /// Every discovered source/scene file (discovery order).
    pub files: Vec<LoadedFile>,
    /// The `project.godot` text, enabling `[autoload]` resolution + version detection.
    pub config: Option<Arc<str>>,
}

impl LoadedProject {
    /// Whether the scan found a Godot project root at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.config.is_none()
    }
}

/// Scan the workspace `roots` (the client's `rootUri` / workspace folders). The project root is the
/// nearest `project.godot` ancestor of the first root that resolves to a real directory; every `.gd`
/// and `.tscn` beneath it is loaded (honoring `.gitignore` + a custom `.gdignore`). Returns an empty
/// project when no `project.godot` is found (the server then behaves as the per-open-file model).
#[must_use]
pub fn load(roots: &[Uri]) -> LoadedProject {
    let Some(root) = roots
        .iter()
        .filter_map(uri_to_path)
        .find_map(|p| find_project_root(&p))
    else {
        return LoadedProject::default();
    };

    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in walk_source_files(&root) {
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen.insert(canon.clone()) {
            continue; // dedup files reached via overlapping symlinks
        }
        let Some(uri) = path_to_uri(&canon) else {
            continue;
        };
        let res_path = res_path_for(&root, &canon);
        if let Ok(bytes) = std::fs::read(&canon) {
            let text: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).into_owned());
            files.push(LoadedFile {
                uri,
                text,
                res_path,
            });
        }
    }
    LoadedProject {
        config: read_project_godot(&root),
        files,
        root: Some(root),
    }
}

/// Walk `root` for `.gd`/`.tscn` files, honoring `.gitignore` + a custom `.gdignore`.
fn walk_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder.add_custom_ignore_filename(".gdignore");
    for entry in builder.build().flatten() {
        let p = entry.path();
        if p.is_file() && is_source(p) {
            out.push(p.to_path_buf());
        }
    }
    out
}

/// Whether a path is a GDScript or scene file (case-insensitive extension).
fn is_source(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gd") || e.eq_ignore_ascii_case("tscn"))
}

/// Walk up from `start` to the nearest ancestor directory containing `project.godot`.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    dir = dir.canonicalize().unwrap_or(dir);
    loop {
        if dir.join("project.godot").is_file() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// The `res://`-relative path of `file` under project `root` (with `/` separators), or `None` when
/// `file` is not under `root`.
#[must_use]
pub fn res_path_for(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let mut s = String::from("res://");
    for (i, comp) in rel.components().enumerate() {
        if i > 0 {
            s.push('/');
        }
        s.push_str(&comp.as_os_str().to_string_lossy());
    }
    Some(s)
}

/// Read a project's `project.godot` text, if present + readable.
fn read_project_godot(root: &Path) -> Option<Arc<str>> {
    std::fs::read(root.join("project.godot"))
        .ok()
        .map(|b| Arc::from(String::from_utf8_lossy(&b).into_owned()))
}

// ---- URI <-> path (file: scheme only) --------------------------------------------------------

/// Convert a `file:` [`Uri`] to a filesystem path, percent-decoding the path and stripping the
/// leading slash before a Windows drive (`/C:/x` → `C:/x`). Returns `None` for a non-`file:` URI.
#[must_use]
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // `file:///path` → rest starts with `/`; `file://host/path` → skip the authority to the first `/`.
    let path_part = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    let decoded = percent_decode(path_part);
    let bytes = decoded.as_bytes();
    // `/C:/...` (a Windows drive path) → drop the leading slash.
    let trimmed = if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
        &decoded[1..]
    } else {
        decoded.as_str()
    };
    Some(PathBuf::from(trimmed))
}

/// Convert a filesystem path to a `file://` [`Uri`], normalizing separators, stripping a Windows
/// `\\?\` verbatim prefix, and percent-encoding reserved characters. Best-effort: returns `None` only
/// if the result fails to parse (it won't for a normal path).
#[must_use]
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with("//?/") {
        s.drain(.."//?/".len()); // strip the Windows `\\?\` verbatim prefix from canonicalize()
    }
    let with_root = if s.starts_with('/') {
        s
    } else {
        format!("/{s}") // Windows "C:/x" → "/C:/x"
    };
    let encoded = percent_encode_path(&with_root);
    format!("file://{encoded}").parse().ok()
}

/// Percent-decode a URI path component (`%XX` → byte); leaves a malformed escape verbatim.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 3 <= bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a path for a `file:` URI: keep the unreserved set + `/` (separators) + `:` (the
/// Windows drive), encode everything else (spaces, `#`, `?`, non-ASCII, …).
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let keep =
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':');
        if keep {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit(u32::from(b >> 4), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit(u32::from(b & 0xf), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

/// A hex digit's value (`0-9`/`a-f`/`A-F`), or `None`.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn uri_path_round_trip_posix() {
        let p = uri_to_path(&uri("file:///home/u/scripts/main.gd")).unwrap();
        assert_eq!(p, PathBuf::from("/home/u/scripts/main.gd"));
        let back = path_to_uri(&p).unwrap();
        assert_eq!(back.as_str(), "file:///home/u/scripts/main.gd");
    }

    #[test]
    fn uri_to_path_strips_windows_drive_slash() {
        let p = uri_to_path(&uri("file:///C:/Users/x/main.gd")).unwrap();
        assert_eq!(p, PathBuf::from("C:/Users/x/main.gd"));
    }

    #[test]
    fn percent_encoded_space_decodes() {
        let p = uri_to_path(&uri("file:///home/my%20game/a.gd")).unwrap();
        assert_eq!(p, PathBuf::from("/home/my game/a.gd"));
        // and re-encodes on the way back out
        assert_eq!(
            path_to_uri(&p).unwrap().as_str(),
            "file:///home/my%20game/a.gd"
        );
    }

    #[test]
    fn res_path_for_under_root() {
        let root = Path::new("/proj");
        assert_eq!(
            res_path_for(root, Path::new("/proj/scripts/main.gd")).as_deref(),
            Some("res://scripts/main.gd")
        );
        assert_eq!(res_path_for(root, Path::new("/elsewhere/x.gd")), None);
    }

    #[test]
    fn is_source_matches_gd_and_tscn() {
        assert!(is_source(Path::new("a.gd")));
        assert!(is_source(Path::new("Main.TSCN")));
        assert!(!is_source(Path::new("a.cs")));
        assert!(!is_source(Path::new("project.godot")));
    }
}
