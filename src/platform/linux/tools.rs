use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use super::lan::{mac_of, parse_mac};
use super::redirects::resolvectl_dns;
use super::command_output;
use crate::tools::{Adapter, Echo, Wifi, WifiState};

pub fn adapters() -> Vec<Adapter> {
    let Some(text) = command_output("ip", &["-j", "addr", "show"]) else { return Vec::new() };
    let Ok(links) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let routes = command_output("ip", &["-j", "route", "show", "default"]).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let mut out = Vec::new();
    for link in links.as_array().into_iter().flatten() {
        let name = link["ifname"].as_str().unwrap_or("").to_string();
        let state = link["operstate"].as_str().unwrap_or("");
        if name.is_empty() || name == "lo" || !(state == "UP" || state == "UNKNOWN") {
            continue;
        }
        let addresses: Vec<(IpAddr, u8)> = link["addr_info"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["local"].as_str()?.parse().ok()?, a["prefixlen"].as_u64()? as u8)))
            .collect();
        let gateways: Vec<IpAddr> = routes
            .as_ref()
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
            .filter(|r| r["dev"].as_str() == Some(&name))
            .filter_map(|r| r["gateway"].as_str()?.parse().ok())
            .collect();
        let dns = dns_servers_of(&name);
        let dhcp = dhcp_on(&name);
        let speed = std::fs::read_to_string(format!("/sys/class/net/{name}/speed")).ok().and_then(|s| s.trim().parse::<i64>().ok()).filter(|s| *s > 0).map_or(0, |mb| mb as u64 * 1_000_000);
        out.push(Adapter {
            description: link["link_type"].as_str().unwrap_or("").to_string(),
            mac: link["address"].as_str().and_then(parse_mac),
            addresses,
            gateways,
            dns,
            dhcp,
            speed,
            name,
        });
    }
    out.sort_by_key(|a| !a.gateways.iter().any(|g| !g.is_unspecified()));
    out
}

// Whether the address came from a DHCP server. NetworkManager knows, and
// where it does not run a lease file for the interface is the other sign.
fn dhcp_on(interface: &str) -> bool {
    if let Some(answer) = command_output("nmcli", &["-g", "IP4.DHCP4", "device", "show", interface]) {
        return answer.lines().any(|l| !l.trim().is_empty());
    }
    ["/var/lib/dhcp", "/var/lib/dhclient", "/var/lib/NetworkManager", "/var/lib/dhcpcd"]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(interface))
}

pub(super) fn dns_servers_of(interface: &str) -> Vec<IpAddr> {
    if let Some(servers) = resolvectl_dns(interface) {
        return servers.iter().filter_map(|s| s.parse().ok()).collect();
    }
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|text| text.lines().filter_map(|l| l.trim().strip_prefix("nameserver")).filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_default()
}

// A whole address, and nothing but https goes out.
pub fn http_get(url: &str) -> Option<String> {
    let body = command_output("curl", &["-s", "-m", "4", "--proto", "=https", url])?;
    let body = body.trim();
    (!body.is_empty()).then(|| body.chars().take(1024).collect())
}

pub fn wifi() -> WifiState {
    let Some(text) = command_output("nmcli", &["-t", "-f", "ACTIVE,SSID,BSSID,SIGNAL,FREQ,RATE,SECURITY,CHAN", "dev", "wifi", "list"]) else {
        return WifiState::NoAdapter;
    };
    // BSSID colons are escaped in terse output: a\:b\:c.
    let Some(line) = text.lines().find(|l| l.starts_with("yes:")) else {
        return if text.trim().is_empty() { WifiState::NoAdapter } else { WifiState::Disconnected };
    };
    let fields: Vec<String> = line.replace("\\:", "\u{1}").split(':').map(|f| f.replace('\u{1}', ":")).collect();
    let field = |i: usize| fields.get(i).map(String::as_str).unwrap_or("");
    let number = |s: &str| s.split_whitespace().next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    let security = field(6).to_uppercase();
    // The Windows numbering the report reads: open, WPA, WPA2, WPA3.
    let (auth, cipher, secured) = match () {
        _ if security.is_empty() || security == "--" => (1, 0, false),
        _ if security.contains("WEP") => (1, 1, true),
        _ if security.contains("WPA3") => (9, 4, true),
        _ if security.contains("WPA2") => (7, 4, true),
        _ if security.contains("WPA") => (4, 2, true),
        _ => (1, 0, true),
    };
    WifiState::Connected(Wifi {
        ssid: field(1).to_string(),
        profile: field(1).to_string(),
        bssid: parse_mac(field(2)).unwrap_or([0; 6]),
        signal: number(field(3)),
        rssi: None,
        frequency: Some(number(field(4))),
        channel: Some(number(field(7))),
        rx_mbps: number(field(5)),
        tx_mbps: number(field(5)),
        auth,
        cipher,
        secured,
    })
}

// The saved key, wrapped the way the report's reader expects it.
pub fn wifi_profile_xml(profile: &str) -> Option<String> {
    let key = command_output("nmcli", &["-s", "-g", "802-11-wireless-security.psk", "connection", "show", profile])?;
    let key = key.trim();
    Some(match key.is_empty() {
        true => "<sharedKey><protected>false</protected></sharedKey>".to_string(),
        false => format!("<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>", xml_escape(key)),
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// One ARP request: a packet nudged at the address makes the kernel ask,
// and its table then has the answer.
pub fn arp(ip: Ipv4Addr, _source: Ipv4Addr) -> Option<[u8; 6]> {
    if let Some(mac) = mac_of(ip) {
        return Some(mac);
    }
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.send_to(&[0], (ip, 9));
    std::thread::sleep(Duration::from_millis(400));
    mac_of(ip)
}

// Echoes through the ping command, which carries the raw-socket right.
pub struct Pinger;

impl Pinger {
    pub fn open() -> Option<Pinger> {
        Some(Pinger)
    }

    pub fn echo(&self, ip: Ipv4Addr, ttl: u8, timeout_ms: u32) -> Option<(Echo, Ipv4Addr, u32)> {
        let wait = timeout_ms.div_ceil(1000).max(1).to_string();
        let out = std::process::Command::new("ping").args(["-c", "1", "-n", "-W", &wait, "-t", &ttl.to_string(), &ip.to_string()]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(rest) = line.split_once(" bytes from ").map(|(_, r)| r) {
                let from: Ipv4Addr = rest.split([':', ' ']).next()?.parse().ok()?;
                let ms = rest.split("time=").nth(1).and_then(|t| t.split_whitespace().next()).and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
                return Some((Echo::Reply, from, ms.round() as u32));
            }
            if let Some(rest) = line.strip_prefix("From ") {
                let from: Ipv4Addr = rest.split([':', ' ']).next()?.parse().ok()?;
                let echo = if rest.contains("Time to live exceeded") { Echo::Expired } else { Echo::Failed };
                return Some((echo, from, 0));
            }
        }
        None
    }
}

pub fn download(url: &str, to: &std::path::Path) -> bool {
    command_output("curl", &["-sSL", "--proto", "=https", "--max-time", "120", "-o", &to.display().to_string(), url]).is_some() && to.is_file()
}

// Nothing to hand a package to here: what put him on the machine takes care
// of replacing him, and it is not his business to do it himself.
pub fn install_package(_path: &std::path::Path) -> bool {
    false
}
