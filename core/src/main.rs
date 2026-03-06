#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cocoon_core::run().await
}
