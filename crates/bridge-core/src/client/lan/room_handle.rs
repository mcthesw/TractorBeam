use std::{io, net::SocketAddr, sync::Arc};

use rand::RngExt as _;
use tokio::runtime::{Builder, Runtime};
use tractor_beam_direct_protocol::{InstanceId, PeerIdentity};

use super::{LanAdapterAddress, LanControlPlane, LanPeerPathState, LanPeerState, LanProbeResult};
use crate::client::{JoinCode, JoinCodeError, LanJoinCode, SessionCredential};

pub struct LanRoomHandle {
    room: Arc<LanControlPlane>,
    runtime: Runtime,
}

impl LanRoomHandle {
    pub fn create(
        steam_id64: u64,
        display_name: String,
        credential: SessionCredential,
        adapters: &[LanAdapterAddress],
    ) -> io::Result<Self> {
        let runtime = lan_runtime()?;
        let room = runtime.block_on(LanControlPlane::create(
            peer_identity(steam_id64),
            display_name,
            credential,
            adapters,
        ))?;
        Ok(Self {
            room: Arc::new(room),
            runtime,
        })
    }

    pub fn join(
        steam_id64: u64,
        display_name: String,
        invitation: &LanJoinCode,
        endpoint: SocketAddr,
        adapters: &[LanAdapterAddress],
    ) -> io::Result<Self> {
        let handle = Self::create(
            steam_id64,
            display_name,
            invitation.session_credential,
            adapters,
        )?;
        handle
            .runtime
            .block_on(handle.room.join(invitation, endpoint))?;
        Ok(handle)
    }

    pub fn probe(invitation: &LanJoinCode) -> io::Result<Vec<LanProbeResult>> {
        lan_runtime().map(|runtime| runtime.block_on(LanControlPlane::probe(invitation)))
    }

    #[must_use]
    pub fn room(&self) -> Arc<LanControlPlane> {
        Arc::clone(&self.room)
    }

    pub fn invitation_code(&self) -> Result<String, JoinCodeError> {
        JoinCode::LanDirect(self.room.invitation()).encode()
    }

    #[must_use]
    pub fn peer_states(&self) -> Vec<LanPeerState> {
        self.room.peer_states()
    }

    #[must_use]
    pub fn path_states(&self) -> Vec<LanPeerPathState> {
        self.room.path_states()
    }

    pub fn stop(&self) {
        self.runtime.block_on(self.room.stop());
    }
}

impl Drop for LanRoomHandle {
    fn drop(&mut self) {
        self.runtime.block_on(self.room.stop());
    }
}

fn lan_runtime() -> io::Result<Runtime> {
    Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tractor-beam-lan")
        .enable_all()
        .build()
}

fn peer_identity(steam_id64: u64) -> PeerIdentity {
    PeerIdentity::new(steam_id64, InstanceId::from_bytes(nonzero_random()))
}

fn nonzero_random() -> [u8; 16] {
    loop {
        let value = rand::rng().random::<[u8; 16]>();
        if value.iter().any(|byte| *byte != 0) {
            return value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_rejects_an_empty_adapter_selection() {
        let result = LanRoomHandle::create(
            1,
            "Player".to_owned(),
            SessionCredential::from_bytes([1; 16]),
            &[],
        );
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidInput));
    }
}
