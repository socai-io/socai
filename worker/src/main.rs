#[tokio::main]
async fn main() -> anyhow::Result<()> {
    socai_worker::run_cli_with_build_info(Default::default()).await
}
