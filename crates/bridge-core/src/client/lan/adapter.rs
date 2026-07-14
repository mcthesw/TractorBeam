use std::{collections::HashSet, io, net::IpAddr};

use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig as _};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanAdapterAddress {
    pub adapter_id: String,
    pub name: String,
    pub address: IpAddr,
    pub interface_index: u32,
}

pub fn enumerate_lan_adapter_addresses() -> io::Result<Vec<LanAdapterAddress>> {
    let interfaces = NetworkInterface::show().map_err(io::Error::other)?;
    let mut seen = HashSet::new();
    let mut addresses = Vec::new();
    for interface in interfaces {
        for raw_address in interface.addr {
            let address = interface_address(raw_address);
            if interface.internal
                || !is_usable_unicast(address)
                || !seen.insert((interface.index, address))
            {
                continue;
            }
            addresses.push(LanAdapterAddress {
                adapter_id: format!("{}:{}", interface.index, interface.name),
                name: interface.name.clone(),
                address,
                interface_index: interface.index,
            });
        }
    }
    addresses.sort_by(|left, right| {
        (left.interface_index, left.address).cmp(&(right.interface_index, right.address))
    });
    Ok(addresses)
}

fn interface_address(address: Addr) -> IpAddr {
    match address {
        Addr::V4(address) => IpAddr::V4(address.ip),
        Addr::V6(address) => IpAddr::V6(address.ip),
    }
}

fn is_usable_unicast(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_broadcast()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_only_removes_invalid_or_same_process_addresses() {
        assert!(!is_usable_unicast("0.0.0.0".parse().unwrap()));
        assert!(!is_usable_unicast("127.0.0.1".parse().unwrap()));
        assert!(!is_usable_unicast("::1".parse().unwrap()));
        assert!(is_usable_unicast("192.168.1.2".parse().unwrap()));
        assert!(is_usable_unicast("100.64.0.1".parse().unwrap()));
        assert!(is_usable_unicast("8.8.8.8".parse().unwrap()));
        assert!(is_usable_unicast("fd00::1".parse().unwrap()));
    }
}
