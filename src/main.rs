#[tokio::main]
async fn main() -> anyhow::Result<()> {
    hunin::interfaces::cli::run().await
}
