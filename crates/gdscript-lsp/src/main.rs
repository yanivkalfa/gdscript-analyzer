//! The `gdscript-lsp` binary: a thin stdio bootstrap that hands the JSON-RPC connection to the
//! server core in [`gdscript_lsp`]. It assumes nothing about Godot and uses no network — pure
//! stdin/stdout, reading the workspace from disk via the VFS.

fn main() -> anyhow::Result<()> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    gdscript_lsp::run(&connection)?;
    io_threads.join()?;
    Ok(())
}
