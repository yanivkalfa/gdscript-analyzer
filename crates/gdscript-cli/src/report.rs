//! Output emitters (Playbook §2). One internal located-diagnostic feeds every format. The column
//! UNIT differs per format: human/GitHub/SARIF use **character** columns; rdjson uses **UTF-8 byte**
//! columns (the §3 [`LineIndex`](crate::lines::LineIndex) exposes both). All positions are 1-based.

use std::io::{self, Write};

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};
use gdscript_base::{Diagnostic, Severity};
use serde_json::json;

use crate::cli::Format;
use crate::engine::{FileDiagnostics, FileSymbols, SourceFile};
use crate::lines::LineCol;

/// A diagnostic with its file + precomputed 1-based start/end positions — the unit every emitter
/// consumes (so position conversion happens exactly once, in [`flatten`]).
struct Located<'a> {
    file: &'a SourceFile,
    diag: &'a Diagnostic,
    start: LineCol,
    end: LineCol,
}

/// Flatten per-file results (already sorted by path, then start offset) into located diagnostics.
fn flatten<'a>(results: &'a [FileDiagnostics<'a>]) -> Vec<Located<'a>> {
    let mut out = Vec::new();
    for fd in results {
        for diag in &fd.diagnostics {
            out.push(Located {
                file: fd.file,
                diag,
                start: fd.file.line_index.line_col(&fd.file.text, diag.range.start),
                end: fd.file.line_index.line_col(&fd.file.text, diag.range.end),
            });
        }
    }
    out
}

/// Emit `results` in `format` to `w`. `color` only affects `human` (machine formats are never
/// colored).
pub fn emit(
    format: Format,
    results: &[FileDiagnostics<'_>],
    color: bool,
    w: &mut dyn Write,
) -> io::Result<()> {
    let located = flatten(results);
    match format {
        Format::Human => human(&located, color, w),
        Format::Json => json_format(&located, w),
        Format::Github => github(&located, w),
        Format::Sarif => sarif(&located, w),
        Format::Rdjson => rdjson(&located, w),
    }
}

// ---- human (rustc-style: severity header + source snippet + caret, via annotate-snippets) ----

/// The lowercase severity word (used by the `json` format; the human format renders its own header).
fn severity_word(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

/// The `annotate-snippets` level for a severity (`Hint` → `NOTE`, which annotate-snippets has).
fn severity_level(sev: Severity) -> Level<'static> {
    match sev {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Info => Level::INFO,
        Severity::Hint => Level::NOTE,
    }
}

/// The rustc/Ruff-style rich human format: a `severity[CODE]: message` header, then the source line
/// with a caret under the offending span, rendered by `annotate-snippets`. Colored (styled renderer)
/// only when `color`; machine formats are never colored. The byte span is clamped to the source so an
/// out-of-bounds (e.g. EOF) range can never panic the renderer.
fn human(located: &[Located<'_>], color: bool, w: &mut dyn Write) -> io::Result<()> {
    let renderer = if color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    for l in located {
        let len = l.file.text.len();
        let span = (l.diag.range.start as usize).min(len)..(l.diag.range.end as usize).min(len);
        let group = severity_level(l.diag.severity)
            .primary_title(l.diag.message.as_str())
            .id(l.diag.code.as_str())
            .element(
                Snippet::source(l.file.text.as_ref())
                    .path(l.file.display.as_str())
                    .line_start(1)
                    .fold(true)
                    .annotation(AnnotationKind::Primary.span(span)),
            );
        writeln!(w, "{}", renderer.render(&[group]))?;
    }
    Ok(())
}

// ---- github (Actions workflow-command annotations) ----

/// `escapeData` (actions/toolkit `command.ts`) — for the message body: `%`→`%25`, CR→`%0D`, LF→`%0A`.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// `escapeProperty` — for property VALUES: the three of `escapeData` PLUS `:`→`%3A`, `,`→`%2C`.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// The Actions annotation command for a severity.
fn github_level(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info | Severity::Hint => "notice",
    }
}

/// `::{level} file=..,line=..,endLine=..,col=..,endColumn=..,title=CODE::message` (1-based char cols).
fn github(located: &[Located<'_>], w: &mut dyn Write) -> io::Result<()> {
    for l in located {
        writeln!(
            w,
            "::{} file={},line={},endLine={},col={},endColumn={},title={}::{}",
            github_level(l.diag.severity),
            escape_property(&l.file.display),
            l.start.line,
            l.end.line,
            l.start.char_col,
            l.end.char_col,
            escape_property(&l.diag.code),
            escape_data(&l.diag.message),
        )?;
    }
    Ok(())
}

// ---- json (stable machine schema) ----

/// An array of diagnostics with both the raw byte range and the 1-based start/end line:col.
fn json_format(located: &[Located<'_>], w: &mut dyn Write) -> io::Result<()> {
    let items: Vec<_> = located
        .iter()
        .map(|l| {
            json!({
                "file": l.file.display,
                "code": l.diag.code,
                "severity": severity_word(l.diag.severity),
                "source": match l.diag.source {
                    gdscript_base::DiagnosticSource::Syntax => "syntax",
                    gdscript_base::DiagnosticSource::Type => "type",
                },
                "message": l.diag.message,
                "range": { "start": l.diag.range.start, "end": l.diag.range.end },
                "start": { "line": l.start.line, "column": l.start.char_col },
                "end": { "line": l.end.line, "column": l.end.char_col },
            })
        })
        .collect();
    serde_json::to_writer_pretty(&mut *w, &items)?;
    writeln!(w)
}

// ---- sarif 2.1.0 (GitHub code scanning) ----

/// SARIF severity → `level` (`error` / `warning` / `note`).
fn sarif_level(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info | Severity::Hint => "note",
    }
}

/// A standard SARIF 2.1.0 document (1-based lines+columns, exclusive end per OASIS). Rules are the
/// distinct diagnostic codes; results carry one physical location each.
fn sarif(located: &[Located<'_>], w: &mut dyn Write) -> io::Result<()> {
    // Distinct rule ids, in first-seen order (deterministic — `located` is already sorted).
    let mut rule_ids: Vec<&str> = Vec::new();
    for l in located {
        if !rule_ids.contains(&l.diag.code.as_str()) {
            rule_ids.push(&l.diag.code);
        }
    }
    let rules: Vec<_> = rule_ids.iter().map(|id| json!({ "id": id })).collect();
    let results: Vec<_> = located
        .iter()
        .map(|l| {
            json!({
                "ruleId": l.diag.code,
                "level": sarif_level(l.diag.severity),
                "message": { "text": l.diag.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": l.file.display },
                        "region": {
                            "startLine": l.start.line,
                            "startColumn": l.start.char_col,
                            "endLine": l.end.line,
                            "endColumn": l.end.char_col,
                        }
                    }
                }],
            })
        })
        .collect();
    let doc = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "gdscript-analyzer",
                "informationUri": "https://github.com/yanivkalfa/gdscript-analyzer",
                "version": env!("CARGO_PKG_VERSION"),
                "rules": rules,
            }},
            "results": results,
        }],
    });
    serde_json::to_writer_pretty(&mut *w, &doc)?;
    writeln!(w)
}

// ---- rdjson (reviewdog DiagnosticResult; 1-based UTF-8 byte columns) ----

/// rdjson severity (`ERROR` / `WARNING` / `INFO`).
fn rdjson_severity(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "ERROR",
        Severity::Warning => "WARNING",
        Severity::Info | Severity::Hint => "INFO",
    }
}

/// A reviewdog `DiagnosticResult` (rdjson). Column is a 1-based UTF-8 **byte** index — uniquely
/// matching our byte-offset core, so this format needs no character projection.
fn rdjson(located: &[Located<'_>], w: &mut dyn Write) -> io::Result<()> {
    let diagnostics: Vec<_> = located
        .iter()
        .map(|l| {
            json!({
                "message": l.diag.message,
                "location": {
                    "path": l.file.display,
                    "range": {
                        "start": { "line": l.start.line, "column": l.start.byte_col },
                        "end": { "line": l.end.line, "column": l.end.byte_col },
                    }
                },
                "severity": rdjson_severity(l.diag.severity),
                "code": { "value": l.diag.code },
            })
        })
        .collect();
    let doc = json!({
        "source": {
            "name": "gdscript-analyzer",
            "url": "https://github.com/yanivkalfa/gdscript-analyzer",
        },
        "diagnostics": diagnostics,
    });
    serde_json::to_writer(&mut *w, &doc)?;
    writeln!(w)
}

// ---- symbols (the `symbols` command — always JSON) ----

/// Emit each file's document outline as a JSON array of `{ file, symbols }`. The `DocumentSymbol`
/// POD serializes directly (byte-offset ranges); consumers map positions via the byte range.
pub fn emit_symbols(results: &[FileSymbols<'_>], w: &mut dyn Write) -> io::Result<()> {
    let items: Vec<_> = results
        .iter()
        .map(|fs| {
            json!({
                "file": fs.file.display,
                "symbols": fs.symbols,
            })
        })
        .collect();
    serde_json::to_writer_pretty(&mut *w, &items)?;
    writeln!(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SourceFile;
    use crate::lines::LineIndex;
    use gdscript_base::{DiagnosticSource, FileId, TextRange};

    fn file(display: &str, text: &str) -> SourceFile {
        SourceFile {
            id: FileId(0),
            display: display.into(),
            line_index: LineIndex::new(text),
            text: text.into(),
            path: None,
            is_target: true,
        }
    }

    fn diag(start: u32, end: u32, sev: Severity, code: &str, msg: &str) -> Diagnostic {
        Diagnostic {
            range: TextRange::new(start, end),
            severity: sev,
            code: code.into(),
            message: msg.into(),
            source: DiagnosticSource::Type,
            fixes: Vec::new(),
        }
    }

    fn render(format: Format, file: &SourceFile, diags: Vec<Diagnostic>) -> String {
        let results = vec![FileDiagnostics {
            file,
            diagnostics: diags,
        }];
        let mut buf = Vec::new();
        emit(format, &results, false, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn human_rich_shows_location_code_message_and_caret() {
        // The rustc-style rich format: a `warning[CODE]: message` header, the file:line:col origin
        // (1-based), the source line, and a caret under the `5 / 2` span (bytes 8..13).
        let f = file("a.gd", "var x = 5 / 2\n");
        let out = render(
            Format::Human,
            &f,
            vec![diag(
                8,
                13,
                Severity::Warning,
                "INTEGER_DIVISION",
                "Integer division.",
            )],
        );
        assert!(out.contains("INTEGER_DIVISION"), "code missing:\n{out}");
        assert!(out.contains("Integer division."), "message missing:\n{out}");
        assert!(out.contains("a.gd:1:9"), "1-based origin missing:\n{out}");
        assert!(out.contains("var x = 5 / 2"), "source line missing:\n{out}");
        assert!(out.contains('^'), "caret underline missing:\n{out}");
    }

    #[test]
    fn github_escapes_property_and_data_distinctly() {
        // A file path with `:` and `,` (property chars) + a message with a newline (data char).
        let f = file("a:b,c.gd", "x\n");
        let out = render(
            Format::Github,
            &f,
            vec![diag(0, 1, Severity::Error, "E,1", "line1\nline2")],
        );
        // property values escape `:`→%3A and `,`→%2C; the message escapes `\n`→%0A but NOT commas.
        assert!(out.contains("file=a%3Ab%2Cc.gd"), "{out}");
        assert!(out.contains("title=E%2C1"), "{out}");
        assert!(out.contains("::line1%0Aline2"), "{out}");
        assert!(out.starts_with("::error "), "{out}");
    }

    #[test]
    fn column_unit_differs_github_char_vs_rdjson_byte() {
        // `é` (2 bytes) precedes the diagnostic, so char col ≠ byte col.
        let f = file("a.gd", "héllo\n"); // h@0, é@1(2B), l@3, l@4, o@5
        let gh = render(
            Format::Github,
            &f,
            vec![diag(3, 5, Severity::Warning, "W", "m")],
        );
        // char col: h, é = 2 chars before offset 3 → col 3.
        assert!(gh.contains("col=3,"), "github should use char col: {gh}");
        let rd = render(
            Format::Rdjson,
            &f,
            vec![diag(3, 5, Severity::Warning, "W", "m")],
        );
        let v: serde_json::Value = serde_json::from_str(&rd).unwrap();
        // byte col: 3 bytes before offset 3 → column 4.
        assert_eq!(
            v["diagnostics"][0]["location"]["range"]["start"]["column"],
            4
        );
        assert_eq!(v["diagnostics"][0]["severity"], "WARNING");
    }

    #[test]
    fn json_carries_byte_range_and_line_col() {
        let f = file("a.gd", "var x = 5 / 2\n");
        let out = render(
            Format::Json,
            &f,
            vec![diag(8, 13, Severity::Warning, "INTEGER_DIVISION", "m")],
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v[0]["range"]["start"], 8);
        assert_eq!(v[0]["start"]["line"], 1);
        assert_eq!(v[0]["start"]["column"], 9);
        assert_eq!(v[0]["severity"], "warning");
    }

    #[test]
    fn sarif_is_2_1_0_with_rules_and_exclusive_end() {
        let f = file("a.gd", "var x = 5 / 2\n");
        let out = render(
            Format::Sarif,
            &f,
            vec![diag(8, 13, Severity::Warning, "INTEGER_DIVISION", "m")],
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["version"], "2.1.0");
        assert_eq!(v["runs"][0]["tool"]["driver"]["name"], "gdscript-analyzer");
        assert_eq!(
            v["runs"][0]["tool"]["driver"]["rules"][0]["id"],
            "INTEGER_DIVISION"
        );
        let region = &v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 1);
        assert_eq!(region["startColumn"], 9); // 1-based
        // `endLine` is INCLUSIVE (line of the end) — for a single-line diagnostic it equals
        // `startLine`, matching clippy/ESLint SARIF + GitHub (NOT startLine+1). `endColumn` is the
        // OASIS-exclusive column-following-the-region.
        assert_eq!(region["endLine"], 1);
        assert_eq!(region["endColumn"], 14); // exclusive end (byte 13 → col 14)
        assert_eq!(v["runs"][0]["results"][0]["level"], "warning");
    }

    #[test]
    fn multiline_diagnostic_uses_end_offset_line_not_plus_one() {
        // A diagnostic spanning line 1 into line 2: bytes 0..5 over "ab\ncd\n" = "ab\ncd".
        // end offset 5 is on line 2 (col 3) — so endLine=2, endColumn=3, NOT line+1=3.
        let f = file("a.gd", "ab\ncd\n");
        let gh = render(
            Format::Github,
            &f,
            vec![diag(0, 5, Severity::Error, "E", "m")],
        );
        assert!(
            gh.contains("line=1,endLine=2,col=1,endColumn=3,"),
            "github multi-line region wrong: {gh}"
        );
        let sarif = render(
            Format::Sarif,
            &f,
            vec![diag(0, 5, Severity::Error, "E", "m")],
        );
        let v: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        let region = &v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 1);
        assert_eq!(region["endLine"], 2);
        assert_eq!(region["endColumn"], 3);
    }

    #[test]
    fn empty_human_is_blank() {
        let f = file("a.gd", "var x = 1\n");
        assert_eq!(render(Format::Human, &f, vec![]), "");
    }
}
