//! Serves the webui and the agent API from one address and opens the browser.
mod app;
mod build;

use std::process::Command;
use tokio::net::TcpListener;

const DEFAULT_ADDR: &str = "127.0.0.1:3080";

async fn run() -> Result<(), String> {
    let no_open = std::env::args().any(|arg| arg == "--no-open");
    let dir = build::static_dir()?;
    let router = app::router(&dir)?;
    let addr = std::env::var("AUGER_WEBUI_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|error| format!("cannot bind {addr}: {error}"))?;
    let url = format!("http://{}", listener.local_addr().map_err(|error| error.to_string())?);
    println!("auger webui at {url}");
    if !no_open {
        open(&url);
    }
    axum::serve(listener, router)
        .await
        .map_err(|error| format!("server error: {error}"))
}

fn open(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(error) = Command::new(opener).arg(url).spawn() {
        eprintln!("cannot open {url} with {opener}: {error}");
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("auger-webui: {error}");
        std::process::exit(1);
    }
}
