mod config;
mod incident;
mod metrics;
mod server;
mod state;

use std::io;

use clap::Parser;

use config::{Args, RelayConfig};

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let config = RelayConfig::load(&args)?;
    server::run(config).await
}
