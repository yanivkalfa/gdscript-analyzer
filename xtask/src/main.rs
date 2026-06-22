//! `xtask` — repository automation for gdscript-analyzer.
//!
//! Run via the cargo alias `cargo xtask <command>` (see `.cargo/config.toml`). This is a plain
//! Rust binary so the automation is cross-platform with no `make`/`bash`/`python` dependency.

mod tasks;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    match cmd {
        "ci" => tasks::ci(),
        "codegen-api" => tasks::codegen_api(),
        "fixtures" => tasks::fixtures(),
        "dist" => tasks::dist(),
        "release" => tasks::release(&args[1..]),
        "" => {
            eprintln!("usage: cargo xtask <ci|codegen-api|fixtures|dist|release>");
            std::process::exit(2);
        }
        other => anyhow::bail!("unknown xtask command: {other:?}"),
    }
}
