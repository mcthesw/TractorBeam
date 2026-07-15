use std::{
    collections::{BTreeMap, HashSet},
    io,
    net::IpAddr,
};

use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig as _};
use thiserror::Error;
use tractor_beam_direct_protocol::MAX_CANDIDATES;

pub const MAX_SELECTED_LAN_ADAPTERS: usize = MAX_CANDIDATES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanAdapterAddress {
    pub adapter_id: String,
    pub name: String,
    pub address: IpAddr,
    pub interface_index: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanAdapter {
    pub adapter_id: String,
    pub name: String,
    pub interface_index: u32,
    pub addresses: Vec<LanAdapterAddress>,
}

impl LanAdapter {
    #[must_use]
    pub fn is_recommended(&self) -> bool {
        self.addresses
            .iter()
            .any(|address| !is_link_local(address.address))
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum LanAdapterSelectionError {
    #[error("at least one LAN adapter is required")]
    Empty,
    #[error("too many LAN adapters selected: {0}")]
    TooMany(usize),
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

pub fn enumerate_lan_adapters() -> io::Result<Vec<LanAdapter>> {
    enumerate_lan_adapter_addresses().map(group_lan_adapter_addresses)
}

#[must_use]
pub fn default_lan_adapters(adapters: &[LanAdapter]) -> Vec<LanAdapter> {
    let mut selected = adapters
        .iter()
        .filter(|adapter| adapter.is_recommended())
        .take(MAX_SELECTED_LAN_ADAPTERS)
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        selected.extend(adapters.iter().take(MAX_SELECTED_LAN_ADAPTERS).cloned());
    }
    selected
}

pub fn lan_candidate_addresses(
    adapters: &[LanAdapter],
) -> Result<Vec<LanAdapterAddress>, LanAdapterSelectionError> {
    if adapters.is_empty() {
        return Err(LanAdapterSelectionError::Empty);
    }
    if adapters.len() > MAX_SELECTED_LAN_ADAPTERS {
        return Err(LanAdapterSelectionError::TooMany(adapters.len()));
    }

    let choices = adapters
        .iter()
        .map(preferred_family_addresses)
        .collect::<Vec<_>>();
    let mut selected = choices
        .iter()
        .filter_map(|addresses| addresses.first().cloned())
        .collect::<Vec<_>>();
    let mut alternatives = choices
        .into_iter()
        .filter_map(|addresses| addresses.get(1).cloned())
        .collect::<Vec<_>>();
    alternatives.sort_by_key(address_sort_key);
    selected.extend(
        alternatives
            .into_iter()
            .take(MAX_CANDIDATES.saturating_sub(selected.len())),
    );
    Ok(selected)
}

fn group_lan_adapter_addresses(addresses: Vec<LanAdapterAddress>) -> Vec<LanAdapter> {
    let mut grouped = BTreeMap::<(u32, String, String), Vec<LanAdapterAddress>>::new();
    for address in addresses {
        grouped
            .entry((
                address.interface_index,
                address.adapter_id.clone(),
                address.name.clone(),
            ))
            .or_default()
            .push(address);
    }
    grouped
        .into_iter()
        .map(|((interface_index, adapter_id, name), mut addresses)| {
            addresses.sort_by_key(address_sort_key);
            LanAdapter {
                adapter_id,
                name,
                interface_index,
                addresses,
            }
        })
        .collect()
}

fn preferred_family_addresses(adapter: &LanAdapter) -> Vec<LanAdapterAddress> {
    let mut ipv4 = adapter
        .addresses
        .iter()
        .filter(|address| address.address.is_ipv4())
        .cloned()
        .collect::<Vec<_>>();
    let mut ipv6 = adapter
        .addresses
        .iter()
        .filter(|address| address.address.is_ipv6())
        .cloned()
        .collect::<Vec<_>>();
    ipv4.sort_by_key(address_sort_key);
    ipv6.sort_by_key(address_sort_key);
    let mut selected = [ipv4.into_iter().next(), ipv6.into_iter().next()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    selected.sort_by_key(address_sort_key);
    selected
}

fn address_sort_key(address: &LanAdapterAddress) -> (u8, IpAddr) {
    let preference = match address.address {
        IpAddr::V4(value) if !value.is_link_local() => 0,
        IpAddr::V6(value) if !value.is_unicast_link_local() => 1,
        IpAddr::V4(_) => 2,
        IpAddr::V6(_) => 3,
    };
    (preference, address.address)
}

fn is_link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_link_local(),
        IpAddr::V6(address) => address.is_unicast_link_local(),
    }
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

    fn address(adapter: u32, name: &str, value: &str) -> LanAdapterAddress {
        LanAdapterAddress {
            adapter_id: format!("{adapter}:{name}"),
            name: name.to_owned(),
            address: value.parse().unwrap(),
            interface_index: adapter,
        }
    }

    #[test]
    fn groups_addresses_and_defaults_to_reachable_adapters() {
        let adapters = group_lan_adapter_addresses(vec![
            address(1, "WLAN", "192.168.31.22"),
            address(1, "WLAN", "2408:8266:3:692d::f52"),
            address(1, "WLAN", "fe80::1"),
            address(2, "Disconnected VPN", "169.254.185.38"),
            address(2, "Disconnected VPN", "fe80::2"),
            address(3, "ZeroTier", "192.168.192.44"),
        ]);

        assert_eq!(adapters.len(), 3);
        assert_eq!(adapters[0].addresses.len(), 3);
        let defaults = default_lan_adapters(&adapters);
        assert_eq!(
            defaults
                .iter()
                .map(|adapter| adapter.name.as_str())
                .collect::<Vec<_>>(),
            ["WLAN", "ZeroTier"]
        );
    }

    #[test]
    fn candidate_plan_uses_one_address_per_adapter_before_second_families() {
        let adapters = group_lan_adapter_addresses(vec![
            address(1, "WLAN", "192.168.31.22"),
            address(1, "WLAN", "2408:8266:3:692d::f52"),
            address(1, "WLAN", "2408:8266:3:692d:ffff::1"),
            address(1, "WLAN", "fe80::1"),
            address(2, "ZeroTier", "192.168.192.44"),
            address(2, "ZeroTier", "fe80::2"),
        ]);

        let candidates = lan_candidate_addresses(&adapters).unwrap();
        assert_eq!(candidates.len(), 4);
        assert_eq!(
            candidates[0].address,
            "192.168.31.22".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            candidates[1].address,
            "192.168.192.44".parse::<IpAddr>().unwrap()
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.address == "2408:8266:3:692d::f52".parse::<IpAddr>().unwrap()
        }));
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.address == "fe80::1".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn default_and_candidate_plan_stay_inside_protocol_budget() {
        let adapters = (0..12)
            .map(|index| LanAdapter {
                adapter_id: format!("{index}:Adapter"),
                name: format!("Adapter {index}"),
                interface_index: index,
                addresses: vec![address(index, "Adapter", &format!("10.0.0.{}", index + 1))],
            })
            .collect::<Vec<_>>();

        let defaults = default_lan_adapters(&adapters);
        assert_eq!(defaults.len(), MAX_SELECTED_LAN_ADAPTERS);
        assert_eq!(
            lan_candidate_addresses(&defaults).unwrap().len(),
            MAX_CANDIDATES
        );
        assert_eq!(
            lan_candidate_addresses(&adapters),
            Err(LanAdapterSelectionError::TooMany(12))
        );
    }
}
