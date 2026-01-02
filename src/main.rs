//! Cocoon standalone binary
//!
//! This is the standalone entry point for cocoon when built with --features standalone

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cocoon::run().await
}
