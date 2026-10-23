//! An ACP (Agent Client Protocol) agent over stdio, driving auger-core sessions
//! in-process. The editor spawns the binary and speaks JSON-RPC on its stdin
//! and stdout.

mod jsonrpc;
mod server;
mod session;
mod storage;
mod updates;

use provider::LlmProvider;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

/// What the agent runs sessions with.
pub struct Deps {
    pub provider: Arc<dyn LlmProvider>,
    pub model: String,
    /// Where session traces are kept. Without it sessions live in memory and
    /// cannot be loaded again.
    pub sessions_dir: Option<PathBuf>,
    /// System prompt. Each session appends its working directory.
    pub system_prompt: String,
}

/// Serve ACP on `input` and `output` until the input ends and every running
/// prompt has settled.
pub fn run(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    deps: Deps,
) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let (tx, rx) = mpsc::channel();
    let reader_tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(input).lines() {
            match line {
                Ok(line) => {
                    if reader_tx.send(server::Inbound::Line(line)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = reader_tx.send(server::Inbound::InputClosed);
    });
    let mut server = server::Server::new(jsonrpc::Out::new(output), deps, runtime.handle().clone(), tx);
    server.serve(rx);
    server.shutdown();
    Ok(())
}
