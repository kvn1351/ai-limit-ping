#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ai_limit_ping::run_cli().await
}
