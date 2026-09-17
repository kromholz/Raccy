use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::watch::Finding;

const LOOK_EVERY: Duration = Duration::from_secs(30);
const NETWORKS_KEPT: usize = 32;

pub struct Look {
    pub(crate) hosts: Option<BTreeMap<String, String>>,
    pub(crate) proxy: Option<String>,
    pub(crate) dns: Vec<(String, String, Vec<String>)>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Known {
    #[serde(default)]
    hosts: Option<BTreeSet<String>>,
    #[serde(default)]
    pub(crate) proxy: Option<String>,
    #[serde(default)]
    dns: HashMap<String, Vec<String>>,
}

pub fn start() -> Receiver<Look> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("raccy-redirects".into())
        .spawn(move || {
            while tx.send(crate::platform::redirects::look()).is_ok() {
                std::thread::sleep(LOOK_EVERY);
            }
        })
        .expect("spawn redirect watch");
    rx
}

pub(crate) fn parse_hosts(text: &str) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split('#').next().unwrap_or_default().split_whitespace();
        let Some(ip) = words.next().filter(|ip| ip.parse::<std::net::IpAddr>().is_ok()) else { continue };
        for name in words {
            names.entry(name.to_lowercase()).or_insert_with(|| ip.to_string());
        }
    }
    names
}

// Docker rewrites its own hosts entry on every network.
pub fn notice(known: &mut Known, look: &Look) -> Vec<Finding> {
    let mut found = Vec::new();
    if let Some(hosts) = &look.hosts {
        let names: BTreeSet<String> = hosts.keys().cloned().collect();
        if let Some(before) = &known.hosts {
            let new: Vec<&String> = names.difference(before).collect();
            if let Some(first) = new.first() {
                found.push(Finding::HostsAdded { name: (*first).clone(), ip: hosts[*first].clone(), more: new.len() - 1 });
            }
        }
        known.hosts = Some(names);
    }
    if let Some(proxy) = &look.proxy {
        if known.proxy.as_ref().is_some_and(|before| before != proxy) && !proxy.is_empty() {
            found.push(Finding::ProxySet { via: proxy.clone() });
        }
        known.proxy = Some(proxy.clone());
    }
    if known.dns.len() > NETWORKS_KEPT {
        known.dns.clear();
    }
    for (network, adapter, servers) in &look.dns {
        if let Some(before) = known.dns.insert(network.clone(), servers.clone())
            && before != *servers
            && !servers.is_empty()
        {
            found.push(Finding::DnsChanged { adapter: adapter.clone(), servers: servers.join(", ") });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_names_are_read_without_comments() {
        let names = parse_hosts("# comment\n127.0.0.1 localhost\nnot an entry, just words\n10.1.2.3  Bank.example host2 # tail\n\n  ::1 localhost\n");
        assert_eq!(names.get("bank.example").map(String::as_str), Some("10.1.2.3"));
        assert_eq!(names.get("host2").map(String::as_str), Some("10.1.2.3"));
        assert_eq!(names.get("localhost").map(String::as_str), Some("127.0.0.1"), "the first line for a name counts");
        assert_eq!(names.len(), 3);
    }

    fn look(hosts: &[(&str, &str)], proxy: &str, dns: &[&str]) -> Look {
        Look {
            hosts: Some(hosts.iter().map(|(name, ip)| (name.to_string(), ip.to_string())).collect()),
            proxy: Some(proxy.to_string()),
            dns: vec![("Wi-Fi|192.168.1.1".into(), "Wi-Fi".into(), dns.iter().map(|s| s.to_string()).collect())],
        }
    }

    #[test]
    fn reroutes_are_told_after_a_quiet_first_look() {
        let mut known = Known::default();
        assert!(notice(&mut known, &look(&[("host.docker.internal", "192.168.1.5")], "", &["192.168.1.1"])).is_empty(), "learned quietly");
        assert!(notice(&mut known, &look(&[("host.docker.internal", "10.0.0.7")], "", &["192.168.1.1"])).is_empty(), "a known name moving is no news");
        let found = notice(&mut known, &look(&[("host.docker.internal", "10.0.0.7"), ("bank.example", "6.6.6.6")], "127.0.0.1:8080", &["6.6.6.53"]));
        assert_eq!(
            found,
            vec![
                Finding::HostsAdded { name: "bank.example".into(), ip: "6.6.6.6".into(), more: 0 },
                Finding::ProxySet { via: "127.0.0.1:8080".into() },
                Finding::DnsChanged { adapter: "Wi-Fi".into(), servers: "6.6.6.53".into() },
            ]
        );
        assert!(notice(&mut known, &look(&[("bank.example", "6.6.6.6")], "", &["6.6.6.53"])).is_empty(), "a proxy turned off and a name gone are no news");
        let other = Look { dns: vec![("Wi-Fi|10.0.0.1".into(), "Wi-Fi".into(), vec!["10.0.0.1".into()])], ..look(&[("bank.example", "6.6.6.6")], "", &[]) };
        assert!(notice(&mut known, &other).is_empty(), "another network is learned quietly");
    }

    #[test]
    #[ignore]
    fn redirects_live() {
        let look = look_now();
        println!("hosts {:?}\nproxy {:?}\ndns {:?}", look.hosts, look.proxy, look.dns);
    }

    fn look_now() -> Look {
        crate::platform::redirects::look()
    }
}
