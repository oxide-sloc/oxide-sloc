mod config;
mod http_client;
mod protocol;
mod server;
mod tools;

use config::McpConfig;
use protocol::{McpRequest, McpResponse};
use server::McpServer;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Tracing goes to stderr — stdout is reserved for the MCP wire protocol.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = McpConfig::from_env();
    let srv = McpServer::new(config);

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut out = BufWriter::new(tokio::io::stdout());

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        let response: Option<McpResponse> = match serde_json::from_str::<McpRequest>(&line) {
            // Notifications dispatch to `None` — process side effects, emit no response.
            Ok(req) => srv.dispatch(req).await,
            Err(e) => Some(McpResponse::parse_error(&e)),
        };
        // Nothing to write for notifications; JSON-RPC forbids answering them.
        if let Some(response) = response {
            let mut json = serde_json::to_string(&response)?;
            json.push('\n');
            out.write_all(json.as_bytes()).await?;
            out.flush().await?;
        }
    }

    Ok(())
}
