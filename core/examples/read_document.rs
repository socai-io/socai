//! Read a real local document through the same tool used by the desktop agent.
//! Usage: cargo run -p socai-core --example read_document -- FILE [OFFSET LIMIT]
use anyhow::{Context, Result};
use serde_json::json;
use socai_core::agent::{ReadFileTool, Tool, ToolContext};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().context("Pass a real document path")?;
    let mut input = json!({"path": path});
    if let Some(offset) = args.next() {
        input["offset"] = json!(offset.parse::<u64>()?);
    }
    if let Some(limit) = args.next() {
        input["limit"] = json!(limit.parse::<u64>()?);
    }
    anyhow::ensure!(args.next().is_none(), "Unexpected extra arguments");
    let ctx = ToolContext::new("read-document", std::env::current_dir()?);
    let result = ReadFileTool::unrestricted().call(input, &ctx).await?;
    println!("{}", result.flat_text());
    Ok(())
}
