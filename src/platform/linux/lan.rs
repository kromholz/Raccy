use std::net::Ipv4Addr;

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

fn arp_table() -> Vec<(Ipv4Addr, [u8; 6], String)> {
    arp_entries(&std::fs::read_to_string("/proc/net/arp").unwrap_or_default())
}

fn arp_entries(text: &str) -> Vec<(Ipv4Addr, [u8; 6], String)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (ip, flags, mac, dev) = (f.first()?, f.get(2)?, f.get(3)?, f.get(5)?);
            // ATF_COM: the entry is complete.
            if u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok()? & 0x2 == 0 {
                return None;
            }
            Some((ip.parse().ok()?, parse_mac(mac)?, dev.to_string()))
        })
        .collect()
}

pub(super) fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let bytes: Vec<u8> = text.split(':').filter_map(|b| u8::from_str_radix(b, 16).ok()).collect();
    bytes.try_into().ok()
}

pub(super) fn default_gateway() -> Option<(Ipv4Addr, String)> {
    best_default_route(&std::fs::read_to_string("/proc/net/route").ok()?)
}

// The routing table as the kernel writes it: addresses in hexadecimal and
// the wrong way round, one route a line. Of the routes to everywhere, the
// one with the lowest metric is the way out.
fn best_default_route(text: &str) -> Option<(Ipv4Addr, String)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (dev, dest, gateway, flags, metric) = (f.first()?, f.get(1)?, f.get(2)?, f.get(3)?, f.get(6)?);
            // RTF_UP and RTF_GATEWAY, to the whole internet.
            if *dest != "00000000" || u32::from_str_radix(flags, 16).ok()? & 0x3 != 0x3 {
                return None;
            }
            let hop = Ipv4Addr::from(u32::from_str_radix(gateway, 16).ok()?.to_le_bytes());
            Some((metric.parse::<u32>().ok()?, hop, dev.to_string()))
        })
        .min_by_key(|(metric, _, _)| *metric)
        .map(|(_, hop, dev)| (hop, dev))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARP: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         aa:bb:cc:dd:ee:ff     *        wlan0
192.168.1.55     0x1         0x0         00:00:00:00:00:00     *        wlan0
10.0.0.9         0x1         0x2         02:42:ac:11:00:02     *        docker0
";

    #[test]
    fn a_neighbour_counts_once_the_kernel_has_its_address() {
        let seen = arp_entries(ARP);
        assert_eq!(seen.len(), 2, "the incomplete entry is nobody: {seen:?}");
        assert_eq!(seen[0].0, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(seen[0].1, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(seen[0].2, "wlan0");
        assert_eq!(seen[1].2, "docker0");
        assert!(arp_entries("").is_empty());
        assert!(arp_entries("IP address       HW type     Flags\n").is_empty());
    }

    #[test]
    fn the_way_out_is_the_default_route_of_the_lowest_metric() {
        let route = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
enp3s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
";
        assert_eq!(best_default_route(route), Some((Ipv4Addr::new(192, 168, 1, 1), "enp3s0".into())));
        let down = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\n\
wlan0\t00000000\t0101A8C0\t0000\t0\t0\t600\n\
wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\n";
        assert_eq!(best_default_route(down), None);
        assert_eq!(best_default_route(""), None);
    }

    #[test]
    fn a_mac_is_six_bytes_or_nothing() {
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff"), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(parse_mac("00:00:00:00:00:00"), Some([0; 6]));
        assert_eq!(parse_mac("aa:bb:cc"), None, "too few");
        assert_eq!(parse_mac("no such thing"), None);
    }
}
