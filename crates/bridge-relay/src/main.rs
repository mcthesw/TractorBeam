mod build_info;
mod config;
mod domain;
mod domain_v2;
mod metrics_v2;
mod peer_registry;
mod server_v2;
mod state_v2;
mod telemetry;
mod v2;

use std::io;

use clap::Parser;

use config::{Args, RelayConfig};

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    if args.should_print_version() {
        println!("tractor-beam-relay {}", build_info::version_label());
        return Ok(());
    }
    let config = RelayConfig::load(&args)?;
    let telemetry = telemetry::RelayTelemetry::init(config.telemetry.as_ref())?;
    tracing::info!(version = %build_info::version_label(), "relay starting");
    let result = tokio::select! {
        result = server_v2::run(config, telemetry.metrics.clone()) => result,
        signal = tokio::signal::ctrl_c() => signal,
    };
    telemetry.shutdown().await;
    result
}
