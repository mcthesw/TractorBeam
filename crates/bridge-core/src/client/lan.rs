//! Direct LAN listener, invitation, and membership control.

mod adapter;
mod control;

pub use adapter::{LanAdapterAddress, enumerate_lan_adapter_addresses};
pub use control::{LanControlPlane, LanProbeResult};
