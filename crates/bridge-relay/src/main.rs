mod build_info;
mod config;
mod domain;
mod domain_v2;
mod metrics_v2;
mod peer_registry;
mod server_v2;
mod state_v2;
mod v2;

use std::io;

use clap::Parser;

use config::{Args, RelayConfig};

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    if args.should_print_version() {
        println!("tractor-beam-relay {}", build_info::version_label());
        return Ok(());
    }
    tracing::info!(
        version = %build_info::version_label(),
        "relay starting"
    );
    let config = RelayConfig::load(&args)?;
    server_v2::run(config).await
}
