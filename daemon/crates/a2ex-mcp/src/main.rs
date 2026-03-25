use a2ex_mcp::build_server_from_env;
use rmcp::{ServiceExt, transport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = build_server_from_env();
    let running = server.serve(transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
