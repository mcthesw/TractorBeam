mod build_info;
mod config;
mod domain;
mod metrics;
mod peer_registry;
mod protocol;
mod server;
mod state;
mod telemetry;

use std::io;

use config::{Args, RelayConfig};

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Args = argh::from_env();
    if args.should_print_version() {
        println!("tractor-beam-relay {}", build_info::version_label());
        return Ok(());
    }
    let config = RelayConfig::load(&args)?;
    let log_format = telemetry::LogFormat::from_env()?;
    let telemetry = telemetry::RelayTelemetry::init(config.telemetry.as_ref(), log_format)?;
    tracing::info!(version = %build_info::version_label(), "relay starting");
    let result = tokio::select! {
        result = server::run(config, telemetry.metrics.clone()) => result,
        signal = tokio::signal::ctrl_c() => signal,
    };
    telemetry.shutdown().await;
    result
}
