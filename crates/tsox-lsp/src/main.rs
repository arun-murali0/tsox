//! `tsox-lsp`: the tsox language server, over stdio.

use std::process::ExitCode;

use tower_lsp_server::Server;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "tsox-lsp [--stdio]\n\n\
    Serves the Language Server Protocol on standard input and output.\n\
    Logging goes to standard error; set TSOX_LSP_LOG (for example `debug`) to change its level.";

#[tokio::main]
async fn main() -> ExitCode {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            // Editors pass this; stdio is the only transport.
            "--stdio" => {}
            "--version" | "-V" => {
                println!("tsox-lsp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tsox-lsp: unknown argument `{other}`\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    // Standard output carries the protocol, so logs go to standard error.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_env("TSOX_LSP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let (service, socket) = tsox_lsp::server::service();
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
    ExitCode::SUCCESS
}
