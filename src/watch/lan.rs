use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use crate::tools::{is_random, mac_text};
use crate::tuning::tuning;
use crate::watch::Finding;

const LOOK_GAP_MAX: u64 = 120;
const DEVICES_KEPT: usize = 256;
const NETWORKS_KEPT: usize = 16;
const NAMES_KEPT: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Neighbour {
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Sighting {
    pub gateway: Ipv4Addr,
    pub neighbours: Vec<Neighbour>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Network {
    pub since: u64,
    pub last: u64,
    pub gateway: String,
    #[serde(default)]
    pub watched: u64,
    pub devices: HashMap<String, Device>,
    #[serde(default)]
    pub new_gateway: Option<(String, u32)>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Device {
    pub first: u64,
    pub last: u64,
    pub ip: String,
    #[serde(default)]
    pub name: Option<String>,
}

pub fn start() -> Receiver<Sighting> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("raccy-lan".into())
        .spawn(move || {
            let mut names: HashMap<[u8; 6], Option<String>> = HashMap::new();
            loop {
                if let Some(mut sighting) = crate::platform::lan::neighbours() {
                    if names.len() > NAMES_KEPT {
                        names.clear();
                    }
                    // A resolver out on the internet would learn every phone and
                    // printer here: names are asked only of one on this network.
                    let dns = crate::tools::dns_servers();
                    let local_dns = !dns.is_empty() && dns.iter().all(|ip| crate::net::place_of(*ip) == Some(crate::net::Place::Lan));
                    for n in &mut sighting.neighbours {
                        let ip = IpAddr::V4(n.ip);
                        n.name = match local_dns {
                            true => names.entry(n.mac).or_insert_with(|| crate::net::reverse_lookup(ip)).clone(),
                            false => names.get(&n.mac).cloned().flatten(),
                        };
                    }
                    if tx.send(sighting).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(tuning().lan.look_every_secs));
            }
        })
        .expect("spawn lan watch");
    rx
}

pub(crate) fn machine(ip: Ipv4Addr, mac: [u8; 6]) -> bool {
    mac[0] & 1 == 0 && mac != [0; 6] && !ip.is_multicast() && !ip.is_broadcast() && !ip.is_unspecified()
}

pub fn carry_over(networks: &mut HashMap<String, Network>, now: u64) {
    for net in networks.values_mut().filter(|net| net.watched == 0 && now.saturating_sub(net.since) >= 24 * 3600) {
        net.watched = tuning().lan.learning_secs;
    }
}

pub fn notice(networks: &mut HashMap<String, Network>, s: &Sighting, now: u64) -> Vec<Finding> {
    let Some(gateway) = s.neighbours.iter().find(|n| n.ip == s.gateway) else { return Vec::new() };
    let key = mac_text(gateway.mac);
    let others = || s.neighbours.iter().filter(|n| n.ip != s.gateway);
    let mut found = Vec::new();
    if !networks.contains_key(&key) {
        let known_here = |net: &Network| {
            if net.gateway != s.gateway.to_string() {
                return 0;
            }
            others().filter(|n| net.devices.contains_key(&mac_text(n.mac))).count()
        };
        // A gateway MAC that belongs to a machine known on that network: it is
        // posing as the gateway. A router answering on a second address of
        // its own is not.
        let posing = others().find(|n| n.mac == gateway.mac).filter(|posing| {
            let mac = mac_text(posing.mac);
            networks.values().any(|net| net.gateway == s.gateway.to_string() && net.devices.contains_key(&mac))
        });
        if let Some(posing) = posing {
            return vec![Finding::GatewaySpoof { gateway: s.gateway, mac: key, posing: posing.ip, name: posing.name.clone() }];
        }
        let moved = networks.iter().find(|(_, net)| net.watched >= tuning().lan.learning_secs && known_here(net) >= 2).map(|(k, _)| k.clone());
        match moved {
            Some(old) => {
                let Some(net) = networks.get_mut(&old) else { return found };
                let looks = match &net.new_gateway {
                    Some((mac, looks)) if *mac == key => looks + 1,
                    _ => 1,
                };
                if looks < tuning().lan.new_gateway_looks {
                    net.new_gateway = Some((key, looks));
                    return found;
                }
                if let Some(mut net) = networks.remove(&old) {
                    net.new_gateway = None;
                    networks.insert(key.clone(), net);
                    found.push(Finding::GatewayChanged { gateway: s.gateway, mac: key.clone() });
                }
            }
            None => {
                if networks.len() >= NETWORKS_KEPT {
                    let stalest = networks.iter().min_by_key(|(_, net)| net.last).map(|(k, _)| k.clone());
                    networks.remove(&stalest.unwrap_or_default());
                }
                let network = Network { since: now, last: now, gateway: s.gateway.to_string(), devices: HashMap::new(), watched: 0, new_gateway: None };
                networks.insert(key.clone(), network);
            }
        }
    }
    let Some(net) = networks.get_mut(&key) else { return found };
    if let Some((other, _)) = net.new_gateway.take() {
        found.push(Finding::GatewayFlapped { gateway: s.gateway, mac: other });
    }
    net.watched += now.saturating_sub(net.last).min(LOOK_GAP_MAX);
    net.last = now;
    net.gateway = s.gateway.to_string();
    let learning = net.watched < tuning().lan.learning_secs;
    // A network with more machines than are kept would keep "finding" the ones dropped to make room.
    let crowded = net.devices.len() >= DEVICES_KEPT;
    for n in others() {
        let mac = mac_text(n.mac);
        if let Some(device) = net.devices.get_mut(&mac) {
            (device.last, device.ip) = (now, n.ip.to_string());
            if n.name.is_some() {
                device.name.clone_from(&n.name);
            }
            continue;
        }
        if !learning && !crowded {
            found.push(Finding::NewDevice { ip: n.ip, mac: mac.clone(), name: n.name.clone(), random: is_random(n.mac) });
        }
        net.devices.insert(mac, Device { first: now, last: now, ip: n.ip.to_string(), name: n.name.clone() });
    }
    if net.devices.len() > DEVICES_KEPT {
        let mut by_age: Vec<(u64, String)> = net.devices.iter().filter(|(_, d)| d.last < now).map(|(mac, d)| (d.last, mac.clone())).collect();
        by_age.sort();
        let over = net.devices.len() - DEVICES_KEPT;
        for (_, mac) in by_age.into_iter().take(over) {
            net.devices.remove(&mac);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;
    const ROUTER: [u8; 6] = [0x10, 0x00, 0x00, 0x00, 0x00, 0x01];
    const LAPTOP: [u8; 6] = [0x10, 0x00, 0x00, 0x00, 0x00, 0x02];
    const PRINTER: [u8; 6] = [0x10, 0x00, 0x00, 0x00, 0x00, 0x03];

    fn at(last: u8, mac: [u8; 6]) -> Neighbour {
        Neighbour { ip: Ipv4Addr::new(192, 168, 1, last), mac, name: None }
    }

    fn sighting(neighbours: Vec<Neighbour>) -> Sighting {
        Sighting { gateway: Ipv4Addr::new(192, 168, 1, 1), neighbours }
    }

    fn learned(networks: &mut HashMap<String, Network>, router: [u8; 6]) {
        networks.get_mut(&mac_text(router)).expect("known network").watched = tuning().lan.learning_secs;
    }

    #[test]
    fn a_network_is_learned_by_time_spent_on_it_not_by_the_calendar() {
        let mut networks = HashMap::new();
        assert!(notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP)]), 0).is_empty());
        assert!(notice(&mut networks, &sighting(vec![at(1, ROUTER), at(3, PRINTER)]), DAY).is_empty());
        assert_eq!(networks[&mac_text(ROUTER)].devices.len(), 2);
        assert_eq!(networks[&mac_text(ROUTER)].watched, LOOK_GAP_MAX);
    }

    #[test]
    fn networks_from_before_count_as_learned() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP)]), 0);
        networks.get_mut(&mac_text(ROUTER)).expect("known").watched = 0;
        carry_over(&mut networks, DAY);
        assert_eq!(networks[&mac_text(ROUTER)].watched, tuning().lan.learning_secs);
    }

    #[test]
    fn a_machine_new_to_a_learned_network_is_told_once() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP)]), 0);
        learned(&mut networks, ROUTER);
        let found = notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 30);
        assert_eq!(
            found,
            vec![Finding::NewDevice { ip: Ipv4Addr::new(192, 168, 1, 3), mac: mac_text(PRINTER), name: None, random: false }]
        );
        assert!(notice(&mut networks, &sighting(vec![at(1, ROUTER), at(3, PRINTER)]), 60).is_empty());
    }

    #[test]
    fn a_machine_posing_as_the_gateway_on_a_known_network_is_caught() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 0);
        let found = notice(&mut networks, &sighting(vec![at(1, LAPTOP), at(2, LAPTOP), at(3, PRINTER)]), DAY);
        assert_eq!(
            found,
            vec![Finding::GatewaySpoof {
                gateway: Ipv4Addr::new(192, 168, 1, 1),
                mac: mac_text(LAPTOP),
                posing: Ipv4Addr::new(192, 168, 1, 2),
                name: None,
            }]
        );
        assert_eq!(networks.len(), 1, "the impostor is not learned as a network");
    }

    #[test]
    fn a_new_router_on_a_learned_network_is_said_once_and_the_network_carries_on() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 0);
        learned(&mut networks, ROUTER);
        let router = [0x30, 0, 0, 0, 0, 1];
        let looks = tuning().lan.new_gateway_looks as u64;
        for look in 1..looks {
            let found = notice(&mut networks, &sighting(vec![at(1, router), at(2, LAPTOP), at(3, PRINTER)]), 30 * look);
            assert!(found.is_empty(), "not a new router yet at look {look}: {found:?}");
        }
        let found = notice(&mut networks, &sighting(vec![at(1, router), at(2, LAPTOP), at(3, PRINTER)]), 30 * looks);
        assert_eq!(found, vec![Finding::GatewayChanged { gateway: Ipv4Addr::new(192, 168, 1, 1), mac: mac_text(router) }]);
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[&mac_text(router)].devices.len(), 2, "its machines are still known");
        assert!(notice(&mut networks, &sighting(vec![at(1, router), at(2, LAPTOP)]), 30 * looks + 30).is_empty());
    }

    #[test]
    fn a_gateway_that_answers_from_another_mac_for_a_while_is_a_warning() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 0);
        learned(&mut networks, ROUTER);
        let spoofer = [0x40, 0, 0, 0, 0, 9];
        for look in 1..=3 {
            assert!(notice(&mut networks, &sighting(vec![at(1, spoofer), at(2, LAPTOP), at(3, PRINTER)]), 30 * look).is_empty());
        }
        assert_eq!(networks.len(), 1, "the network is not re-keyed under the other MAC");
        let found = notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 120);
        assert_eq!(found, vec![Finding::GatewayFlapped { gateway: Ipv4Addr::new(192, 168, 1, 1), mac: mac_text(spoofer) }]);
        assert!(notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP)]), 150).is_empty(), "said once");
    }

    #[test]
    fn a_router_answering_on_a_second_address_of_a_learned_network_is_no_impostor() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP), at(3, PRINTER)]), 0);
        learned(&mut networks, ROUTER);
        let router = [0x30, 0, 0, 0, 0, 1];
        let found = notice(&mut networks, &sighting(vec![at(1, router), at(254, router), at(2, LAPTOP), at(3, PRINTER)]), 30);
        assert!(!found.iter().any(|f| matches!(f, Finding::GatewaySpoof { .. })), "{found:?}");
    }

    #[test]
    fn a_router_with_two_addresses_on_a_new_network_is_no_impostor() {
        let mut networks = HashMap::new();
        notice(&mut networks, &sighting(vec![at(1, ROUTER), at(2, LAPTOP)]), 0);
        let other = [0x20, 0, 0, 0, 0, 1];
        let cafe = sighting(vec![at(1, other), at(254, other), at(7, [0x20, 0, 0, 0, 0, 7])]);
        assert!(notice(&mut networks, &cafe, DAY).is_empty());
        assert_eq!(networks.len(), 2);
    }

    #[test]
    fn broadcasts_are_not_machines() {
        assert!(!machine(Ipv4Addr::new(192, 168, 1, 255), [0xff; 6]));
        assert!(!machine(Ipv4Addr::new(224, 0, 0, 251), [0x01, 0x00, 0x5e, 0, 0, 0xfb]));
        assert!(machine(Ipv4Addr::new(192, 168, 1, 2), LAPTOP));
    }

    #[test]
    #[ignore]
    fn look_live() {
        let s = crate::platform::lan::neighbours().expect("a default route");
        let gateway = s.neighbours.iter().any(|n| n.ip == s.gateway);
        println!("gateway in the table: {gateway}, machines: {}", s.neighbours.len());
    }
}
