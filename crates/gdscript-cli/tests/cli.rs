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
fn strict_flag_surfaces_the_opt_in_group_in_a_project() {
    let dir = temp_project();
    // A `project.godot` is present ⇒ default is engine-defaults (the opt-in UNSAFE_* group ignored).
    write(
        &dir,
        "scripts/main.gd",
        "extends Node\nfunc f(n: Node):\n\tn.no_such_method()\n",
    );
    let o = run(&["check", "--format", "json", dir.to_str().unwrap()]);
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains("UNSAFE_METHOD_ACCESS"),
        "default run must not surface the opt-in group: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o2 = run(&[
        "check",
        "--strict",
        "--format",
        "json",
        dir.to_str().unwrap(),
    ]);
    assert!(
        String::from_utf8_lossy(&o2.stdout).contains("UNSAFE_METHOD_ACCESS"),
        "--strict must surface the opt-in group: {}",
        String::from_utf8_lossy(&o2.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn strict_and_engine_defaults_together_is_a_usage_error() {
    let dir = temp_project();
    write(&dir, "scripts/main.gd", "func f():\n\tpass\n");
    let o = run(&[
        "check",
        "--strict",
        "--engine-defaults",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2, "conflicting flags must be a usage error");
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
fn config_toml_gates_warnings_and_no_config_ignores_it() {
    let dir = temp_project();
    write(
        &dir,
        "scripts/main.gd",
        "func f() -> int:\n\tvar x = 5 / 2\n\treturn x\n",
    );
    // A discovered gdscript-analyzer.toml sets the project floor: warnings fail.
    write(&dir, "gdscript-analyzer.toml", "error_on_warning = true\n");
    let path = dir.to_str().unwrap();
    assert_eq!(
        code(&run(&["check", path])),
        1,
        "config error_on_warning=true gates the warning"
    );
    assert_eq!(
        code(&run(&["check", "--no-config", path])),
        0,
        "--no-config ignores the discovered config"
    );
    // An inline override reaches the same gate without a file.
    write(&dir, "gdscript-analyzer.toml", "error_on_warning = false\n");
    assert_eq!(
        code(&run(&["check", "--config", "error_on_warning=true", path])),
        1,
        "inline --config override gates the warning"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_config_file_exits_2() {
    let dir = temp_project();
    write(&dir, "scripts/main.gd", "func f() -> int:\n\treturn 1\n");
    let path = dir.to_str().unwrap();
    let missing = dir.join("nope.toml");
    assert_eq!(
        code(&run(&[
            "check",
            "--config",
            missing.to_str().unwrap(),
            path
        ])),
        2,
        "an unreadable explicit --config file is a usage/config error"
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
