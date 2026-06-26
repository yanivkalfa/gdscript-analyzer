//! The `gdscript` binary — a thin shell. Parse args (clap exits 2 on a usage error / 0 on
//! `--help`/`--version`), run the library core under a panic guard, and `process::exit` the code.

use clap::Parser;
use gdscript_cli::cli::Cli;

fn main() {
    gdscript_cli::install_panic_hook();
    let cli = Cli::parse();
    // A panic inside a single file's analysis is caught here → a stable internal-error exit code.
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gdscript_cli::run(&cli)))
        .unwrap_or(gdscript_cli::EXIT_INTERNAL);
    std::process::exit(code);
}
