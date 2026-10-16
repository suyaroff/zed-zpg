mod config;
mod db;
mod lsp;
mod state;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use zpg_core::catalog;

#[derive(Parser)]
#[command(version, about = "Browse and edit PostgreSQL tables as text in Zed")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create .zpg/catalog.pgdb in the project and open it in Zed
    Open {
        /// Project root
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Zed CLI to launch
        #[arg(long, env = "ZPG_ZED", default_value = "zed")]
        zed: String,
    },
    /// Run the language server on stdio
    Lsp,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zpg: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Open { dir, zed } => {
            let root = dir.join(".zpg");
            let ignore = root.join(".gitignore");
            if !ignore.exists() {
                state::atomic_write(&ignore, b"*\n")?;
            }
            let path = root.join("catalog.pgdb");
            if !path.exists() {
                // The language server fills it in when Zed opens it.
                state::atomic_write(&path, format!("{}\n", catalog::HEADER).as_bytes())?;
            }
            let status = Command::new(&zed)
                .arg(&path)
                .status()
                .map_err(|e| format!("failed to launch `{zed}`: {e}"))?;
            if !status.success() {
                return Err(format!("`{zed}` exited with {status}"));
            }
        }
        Cmd::Lsp => lsp::run().map_err(|e| e.to_string())?,
    }
    Ok(())
}
