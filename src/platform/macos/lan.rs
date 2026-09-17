use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::command_output;
use crate::watch::lan::{Neighbour, Sighting, machine};

pub fn neighbours() -> Option<Sighting> {
    let (gateway, interface) = default_gateway()?;
    let neighbours = arp_table()
        .into_iter()
        .filter(|(_, _, dev)| *dev == interface)
        .filter_map(|(ip, mac, _)| machine(ip, mac).then_some(Neighbour { ip, mac, name: None }))
        .collect();
    Some(Sighting { gateway, neighbours })
}

pub fn mac_of(ip: Ipv4Addr) -> Option<[u8; 6]> {
    arp_table().into_iter().find(|(there, _, _)| *there == ip).map(|(_, mac, _)| mac)
}

type Entry = (Ipv4Addr, [u8; 6], String);

// Reading the table here means running a program, and the neighbours tool asks
// after two hundred and fifty addresses at once, each on a thread of its own.
// One reading serves them all for as long as it is warm: the first to ask does
// the work while the rest wait behind the same lock and then take the copy.
// Warm for less time than the tool waits for an answer, so the reading after
// the knock is a fresh one.
const TABLE_WARM: Duration = Duration::from_millis(250);
static TABLE: Mutex<Option<(Instant, Vec<Entry>)>> = Mutex::new(None);

fn arp_table() -> Vec<Entry> {
    let mut held = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, table)) = held.as_ref()
        && at.elapsed() < TABLE_WARM
    {
        return table.clone();
    }
    let table = read_table();
    *held = Some((Instant::now(), table.clone()));
    table
}

// `? (192.168.1.1) at 10:2b:aa:7f:9:a2 on en0 ifscope [ethernet]`, with the
// bytes written without their leading zero.
fn read_table() -> Vec<Entry> {
    let Some(text) = command_output("arp", &["-an"]) else { return Vec::new() };
    text.lines().filter_map(parse_arp).collect()
}

fn parse_arp(line: &str) -> Option<Entry> {
    let ip: Ipv4Addr = line.split(['(', ')']).nth(1)?.parse().ok()?;
    let mac = parse_mac(line.split(" at ").nth(1)?.split_whitespace().next()?)?;
    let dev = line.split(" on ").nth(1)?.split_whitespace().next()?.to_string();
    Some((ip, mac, dev))
}

pub(super) fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let bytes: Vec<u8> = text.split(':').filter_map(|b| u8::from_str_radix(b, 16).ok()).collect();
    bytes.try_into().ok()
}

// `route -n get default` says which way out and through what.
pub(super) fn default_gateway() -> Option<(Ipv4Addr, String)> {
    let text = command_output("route", &["-n", "get", "default"])?;
    let field = |name: &str| {
        text.lines().find_map(|l| l.trim().strip_prefix(name)?.trim().strip_prefix(':').map(str::trim).map(str::to_string))
    };
    Some((field("gateway")?.parse().ok()?, field("interface")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neighbour_table_reads_as_macos_writes_it() {
        let line = "? (192.0.2.1) at 10:2b:aa:7f:9:a2 on en0 ifscope [ethernet]";
        let (ip, mac, dev) = parse_arp(line).expect("a whole entry");
        assert_eq!(ip, Ipv4Addr::new(192, 0, 2, 1));
        assert_eq!(mac, [0x10, 0x2b, 0xaa, 0x7f, 0x09, 0xa2], "a byte written without its leading zero");
        assert_eq!(dev, "en0");
        assert_eq!(parse_arp("? (192.0.2.9) at (incomplete) on en0"), None, "nobody answered for it");
    }
}
