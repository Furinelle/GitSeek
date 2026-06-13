#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gitseek::cli::run().await
}
