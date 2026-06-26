//! End-to-end tests of the `gdscript` binary over real temp fixtures — the exit-code contract
//! (Playbook §6) + output. These exercise the only layer that touches the filesystem.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Create a unique temp project dir with a `project.godot` + a `scripts/` subdir.
fn temp_project() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gdscript_cli_e2e_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("project.godot"),
        "[application]\nconfig/name=\"e2e\"\n",
    )
    .unwrap();
    dir
}

fn write(dir: &Path, rel: &str, content: &str) {
    std::fs::write(dir.join(rel), content).unwrap();
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gdscript"))
        .args(args)
        .output()
        .expect("spawn gdscript")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code")
}

#[test]
fn clean_project_exits_0_with_empty_stdout() {
    let dir = temp_project();
    write(&dir, "scripts/main.gd", "func f() -> int:\n\treturn 1\n");
    let o = run(&["check", dir.to_str().unwrap()]);
    assert_eq!(code(&o), 0);
    assert!(
        o.stdout.is_empty(),
        "clean run prints no diagnostics: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn warning_is_non_fatal_but_gateable() {
    let dir = temp_project();
    write(
        &dir,
        "scripts/main.gd",
        "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
    );
    let path = dir.to_str().unwrap();
    assert_eq!(
        code(&run(&["check", path])),
        0,
        "warnings are non-fatal by default"
    );
    assert_eq!(
        code(&run(&["check", "--error-on-warning", path])),
        1,
        "--error-on-warning gates them"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_error_exits_1() {
    let dir = temp_project();
    write(&dir, "scripts/bad.gd", "var x = )\n");
    assert_eq!(code(&run(&["check", dir.to_str().unwrap()])), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lint_drops_hard_errors_keeps_warnings() {
    let dir = temp_project();
    write(&dir, "scripts/bad.gd", "var x = )\n"); // a hard syntax error
    write(
        &dir,
        "scripts/warn.gd",
        "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n", // a warning
    );
    let path = dir.to_str().unwrap();
    let o = run(&["lint", path]);
    assert_eq!(code(&o), 0, "lint warnings are non-fatal");
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("INTEGER_DIVISION"), "{stdout}");
    assert!(
        !stdout.contains("GDSCRIPT_SYNTAX"),
        "lint must drop hard errors: {stdout}"
    );
    assert_eq!(
        code(&run(&["lint", "--error-on-warning", path])),
        1,
        "gate the warning"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn json_output_parses() {
    let dir = temp_project();
    write(
        &dir,
        "scripts/main.gd",
        "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
    );
    let o = run(&["check", "--format", "json", dir.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("valid json");
    assert!(v.is_array());
    assert_eq!(v[0]["code"], "INTEGER_DIVISION");
    assert_eq!(v[0]["start"]["line"], 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn format_passthrough_exits_0() {
    let dir = temp_project();
    write(&dir, "scripts/main.gd", "func f():\n\tpass\n");
    assert_eq!(code(&run(&["format", "--check", dir.to_str().unwrap()])), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_file_writes_machine_output() {
    let dir = temp_project();
    write(
        &dir,
        "scripts/main.gd",
        "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
    );
    let out = dir.join("report.json");
    let o = run(&[
        "check",
        "--format",
        "json",
        "--output-file",
        out.to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(o.stdout.is_empty(), "output went to the file, not stdout");
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(written.contains("INTEGER_DIVISION"), "{written}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_command_is_usage_error_2() {
    assert_eq!(code(&run(&["frobnicate"])), 2);
}
