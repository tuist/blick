#[tokio::main]
async fn main() -> Result<(), blick::error::BlickError> {
    blick::run().await
}
