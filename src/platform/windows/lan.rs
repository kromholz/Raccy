use std::net::Ipv4Addr;

use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIpForwardTable2, GetIpInterfaceEntry, GetIpNetTable2, MIB_IPFORWARD_TABLE2, MIB_IPINTERFACE_ROW, MIB_IPNET_TABLE2,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, NlnsProbe};

use crate::watch::lan::{Neighbour, Sighting, machine};

pub fn neighbours() -> Option<Sighting> {
    let (gateway, interface) = default_gateway()?;
    let mut table: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpNetTable2(AF_INET, &mut table) } != 0 || table.is_null() {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let neighbours = rows
        .iter()
        // Probe and later: an answer came, or is being checked again.
        .filter(|r| r.InterfaceIndex == interface && r.State >= NlnsProbe && r.PhysicalAddressLength == 6)
        .filter_map(|r| {
            let ip = Ipv4Addr::from(unsafe { r.Address.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes());
            let mac: [u8; 6] = r.PhysicalAddress[..6].try_into().ok()?;
            machine(ip, mac).then_some(Neighbour { ip, mac, name: None })
        })
        .collect();
    unsafe { FreeMibTable(table.cast()) };
    Some(Sighting { gateway, neighbours })
}

pub fn mac_of(ip: Ipv4Addr) -> Option<[u8; 6]> {
    let mut table: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpNetTable2(AF_INET, &mut table) } != 0 || table.is_null() {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let mac = rows
        .iter()
        .filter(|r| r.PhysicalAddressLength == 6 && Ipv4Addr::from(unsafe { r.Address.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes()) == ip)
        .find_map(|r| r.PhysicalAddress[..6].try_into().ok());
    unsafe { FreeMibTable(table.cast()) };
    mac
}

fn default_gateway() -> Option<(Ipv4Addr, u32)> {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpForwardTable2(AF_INET, &mut table) } != 0 || table.is_null() {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let best = rows
        .iter()
        .filter(|r| r.DestinationPrefix.PrefixLength == 0)
        .filter_map(|r| {
            let hop = Ipv4Addr::from(unsafe { r.NextHop.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes());
            (!hop.is_unspecified()).then(|| (r.Metric.saturating_add(interface_metric(r.InterfaceIndex)), hop, r.InterfaceIndex))
        })
        .min_by_key(|&(metric, _, _)| metric)
        .map(|(_, hop, interface)| (hop, interface));
    unsafe { FreeMibTable(table.cast()) };
    best
}

// An interface's own metric, which Windows adds to a route's when it picks
// the route to use. An interface it cannot read comes last.
fn interface_metric(index: u32) -> u32 {
    let mut row: MIB_IPINTERFACE_ROW = unsafe { std::mem::zeroed() };
    row.Family = AF_INET;
    row.InterfaceIndex = index;
    match unsafe { GetIpInterfaceEntry(&mut row) } {
        0 => row.Metric,
        _ => u32::MAX / 2,
    }
}
