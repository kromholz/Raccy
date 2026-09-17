use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use super::command_output;
use super::lan::{mac_of, parse_mac};
use crate::tools::{Adapter, Echo, Wifi, WifiState};

pub fn adapters() -> Vec<Adapter> {
    let Some(text) = command_output("ifconfig", &["-a"]) else { return Vec::new() };
    let mut out: Vec<Adapter> = parse_ifconfig(&text);
    let names: Vec<String> = out.iter().map(|a| a.name.clone()).collect();
    let routes = command_output("netstat", &["-rn"]).map(|text| default_routes(&text, &names)).unwrap_or_default();
    let resolvers = command_output("scutil", &["--dns"]).map(|text| parse_scutil_dns(&text)).unwrap_or_default();
    for adapter in &mut out {
        adapter.gateways = routes.iter().filter(|(name, _)| *name == adapter.name).map(|(_, gateway)| *gateway).collect();
        adapter.dns = resolvers.iter().find(|(name, _)| *name == adapter.name).map(|(_, dns)| dns.clone()).unwrap_or_else(resolv_conf);
        adapter.dhcp = dhcp_on(&adapter.name);
    }
    out.sort_by_key(|a| !a.gateways.iter().any(|g| !g.is_unspecified()));
    out
}

// A line with nothing before the name opens an interface, and everything
// indented under it belongs to that one.
fn parse_ifconfig(text: &str) -> Vec<Adapter> {
    let mut out: Vec<Adapter> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(char::is_whitespace) else {
            let Some((name, _)) = line.split_once(':') else { continue };
            out.push(Adapter {
                name: name.to_string(),
                description: String::new(),
                mac: None,
                addresses: Vec::new(),
                gateways: Vec::new(),
                dns: Vec::new(),
                dhcp: false,
                speed: 0,
            });
            continue;
        };
        let Some(adapter) = out.last_mut() else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        // The words that open a line carry a colon where ifconfig writes one.
        match f.first().map(|w| w.trim_end_matches(':')) {
            Some("ether") => adapter.mac = f.get(1).and_then(|m| parse_mac(m)),
            Some("status") => adapter.description = f.get(1).unwrap_or(&"").to_string(),
            Some("media") => adapter.speed = media_speed(rest),
            Some("inet") => {
                if let (Some(ip), Some(mask)) = (f.get(1).and_then(|a| a.parse().ok()), f.get(3)) {
                    adapter.addresses.push((IpAddr::V4(ip), prefix_of(mask)));
                }
            }
            Some("inet6") => {
                let addr = f.get(1).map(|a| a.split('%').next().unwrap_or(a));
                if let (Some(Ok(ip)), Some(prefix)) = (addr.map(str::parse), f.iter().position(|w| *w == "prefixlen").and_then(|at| f.get(at + 1))) {
                    adapter.addresses.push((IpAddr::V6(ip), prefix.parse().unwrap_or(0)));
                }
            }
            _ => {}
        }
    }
    out.retain(|a| a.name != "lo0" && a.description == "active" && !a.addresses.is_empty());
    out
}

// `netmask 0xffffff00`, which counts the same as a prefix of 24.
fn prefix_of(mask: &str) -> u8 {
    u32::from_str_radix(mask.trim_start_matches("0x"), 16).map(u32::count_ones).unwrap_or(0) as u8
}

// `media: autoselect (1000baseT <full-duplex>)`, where a wireless one says
// nothing of the sort and its rate comes from the network instead.
fn media_speed(line: &str) -> u64 {
    let Some(before) = line.split("base").next().filter(|b| *b != line) else { return 0 };
    let digits: String = before.chars().rev().take_while(char::is_ascii_digit).collect();
    digits.chars().rev().collect::<String>().parse::<u64>().unwrap_or(0) * 1_000_000
}

// `default  192.168.1.1  UGScg  en0`, where the columns have moved between
// releases: the gateway is the address and the interface is the name.
fn default_routes(text: &str, names: &[String]) -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| l.split_whitespace().next() == Some("default")) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = f.iter().find(|w| names.iter().any(|n| n == *w)) else { continue };
        // A default route out of a tunnel has a name there and no address.
        if let Some(gateway) = f.get(1).and_then(|g| g.split('%').next()).and_then(|g| g.parse().ok()) {
            out.push((name.to_string(), gateway));
        }
    }
    out
}

// `scutil --dns` prints one resolver after another; the servers of each are
// numbered, and the interface it belongs to is named beside its index.
fn parse_scutil_dns(text: &str) -> Vec<(String, Vec<IpAddr>)> {
    let mut out: Vec<(String, Vec<IpAddr>)> = Vec::new();
    let mut servers: Vec<IpAddr> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("resolver #") {
            servers.clear();
            continue;
        }
        if let Some(server) = line.strip_prefix("nameserver[").and_then(|rest| rest.split_once(" : ")).and_then(|(_, ip)| ip.trim().parse().ok()) {
            servers.push(server);
            continue;
        }
        // `if_index : 12 (en0)`, the last thing said about a resolver.
        if let Some(name) = line.strip_prefix("if_index").and_then(|rest| rest.split(['(', ')']).nth(1))
            && !servers.is_empty()
            && !out.iter().any(|(had, _)| had == name)
        {
            out.push((name.to_string(), std::mem::take(&mut servers)));
        }
    }
    out
}

fn resolv_conf() -> Vec<IpAddr> {
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|text| text.lines().filter_map(|l| l.trim().strip_prefix("nameserver")).filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_default()
}

// A DHCP server's answer is kept for as long as the lease lasts, and there is
// none at all for an address somebody typed in.
fn dhcp_on(interface: &str) -> bool {
    command_output("ipconfig", &["getpacket", interface]).is_some_and(|said| said.contains("op ="))
}

// A whole address, and nothing but https goes out.
pub fn http_get(url: &str) -> Option<String> {
    let body = command_output("curl", &["-s", "-m", "4", "--proto", "=https", url])?;
    let body = body.trim();
    (!body.is_empty()).then(|| body.chars().take(1024).collect())
}

// A Mac hands the name of the network to nobody without location. The asking
// happens where the tools cannot do it themselves: the window belongs to the
// system and only the main thread may raise it, so the tool leaves word here
// and the loop that draws him picks it up.
pub(super) static WANTS_LOCATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// CoreLocation answers a delegate and nobody else. Without one the question
// is asked into the void and the window never comes.
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager, CLLocationManagerDelegate};

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RaccyLocation"]
    struct Asking;

    unsafe impl NSObjectProtocol for Asking {}

    unsafe impl CLLocationManagerDelegate for Asking {
        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn changed(&self, manager: &CLLocationManager) {
            let status = unsafe { manager.authorizationStatus() };
            crate::trace::record(|| format!("location: answered {status:?}"));
            // Whatever the answer, he is not here for the whereabouts: the
            // name of the network is the whole of it, and he goes back to
            // being a thing on the desktop rather than a program in the Dock.
            if status != CLAuthorizationStatus::NotDetermined {
                unsafe { manager.stopUpdatingLocation() };
                if let Some(mtm) = MainThreadMarker::new() {
                    super::appkit::stand_aside(mtm);
                }
            }
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn failed(&self, _manager: &CLLocationManager, error: &objc2_foundation::NSError) {
            crate::trace::record(|| format!("location: refused, {}", error.localizedDescription()));
        }
    }
);

// The manager and its delegate have to outlive the asking: one that is let go
// of takes the question with it and no window ever appears.
pub(super) fn ask_for_location(mtm: MainThreadMarker) {
    thread_local! {
        static HELD: std::cell::OnceCell<(Retained<CLLocationManager>, Retained<Asking>)> = const { std::cell::OnceCell::new() };
    }
    HELD.with(|held| {
        let (manager, _) = held.get_or_init(|| {
            let manager = unsafe { CLLocationManager::new() };
            let asking: Retained<Asking> = unsafe { msg_send![Asking::alloc(mtm), init] };
            unsafe { manager.setDelegate(Some(ProtocolObject::from_ref(&*asking))) };
            (manager, asking)
        });
        let status = unsafe { manager.authorizationStatus() };
        crate::trace::record(|| format!("location: status {status:?}"));
        if status == CLAuthorizationStatus::NotDetermined {
            // The window the system puts up belongs to whoever is in front,
            // and an accessory is in front of nobody: he steps into the Dock
            // for as long as the question is open and then steps back.
            super::appkit::step_forward(mtm);
            unsafe { manager.requestWhenInUseAuthorization() };
            // On a Mac the window comes when a program reaches for the
            // whereabouts, not when it asks politely, so it has to reach.
            unsafe { manager.startUpdatingLocation() };
        }
    });
}

// The wireless card, as the system itself reports it. The airport tool that
// used to answer this was taken away, and what is left names the network but
// never its BSSID, which is the one thing here that asks for location.
pub fn wifi() -> WifiState {
    let Some(text) = command_output("system_profiler", &["-json", "SPAirPortDataType"]) else { return WifiState::NoAdapter };
    let Ok(said) = serde_json::from_str::<serde_json::Value>(&text) else { return WifiState::NoAdapter };
    let interfaces = &said["SPAirPortDataType"][0]["spairport_airport_interfaces"];
    let Some(card) = interfaces.as_array().into_iter().flatten().next() else { return WifiState::NoAdapter };
    let network = &card["spairport_current_network_information"];
    let Some(ssid) = network["_name"].as_str() else { return WifiState::Disconnected };
    // The name of the network is one of the things a Mac keeps back from a
    // program with no location access: it hands over a word saying so in its
    // place, and everything else about the network all the same.
    // The name goes to the program that holds location, and to that program
    // only: a child asked on his behalf gets the placeholder all the same.
    let ssid = match ssid {
        "<redacted>" => match ssid_in_process() {
            Some(name) => name,
            None => {
                crate::trace::record(|| "wifi: the name is kept back, asking for location".into());
                WANTS_LOCATION.store(true, std::sync::atomic::Ordering::SeqCst);
                return WifiState::Denied;
            }
        },
        name => name.to_string(),
    };
    let (channel, frequency) = channel_of(network["spairport_network_channel"].as_str().unwrap_or_default());
    let rssi = signal_noise(network["spairport_signal_noise"].as_str().unwrap_or_default());
    let rate = network["spairport_network_rate"].as_f64().unwrap_or(0.0).round() as u32;
    let (auth, cipher, secured) = security_of(network["spairport_security_mode"].as_str().unwrap_or_default());
    WifiState::Connected(Wifi {
        profile: ssid.clone(),
        ssid,
        bssid: [0; 6],
        // The same 0 to 100 the other two report, from the strength in dBm.
        signal: rssi.map_or(0, |dbm| ((dbm + 100) * 2).clamp(0, 100) as u32),
        rssi,
        frequency,
        channel,
        rx_mbps: rate,
        tx_mbps: rate,
        auth,
        cipher,
        secured,
    })
}

fn ssid_in_process() -> Option<String> {
    let client = unsafe { objc2_core_wlan::CWWiFiClient::sharedWiFiClient() };
    let interface = unsafe { client.interface() }?;
    let ssid = unsafe { interface.ssid() }?;
    Some(ssid.to_string())
}

// `149 (5GHz, 80MHz)`: the channel, and which band it is on, which is what
// says where in the spectrum it sits.
fn channel_of(text: &str) -> (Option<u32>, Option<u32>) {
    let Ok(channel) = text.split_whitespace().next().unwrap_or_default().parse::<u32>() else { return (None, None) };
    let frequency = match () {
        _ if text.contains("6GHz") => 5950 + channel * 5,
        _ if text.contains("5GHz") => 5000 + channel * 5,
        _ if channel == 14 => 2484,
        _ => 2407 + channel * 5,
    };
    (Some(channel), Some(frequency))
}

// `-45 dBm / -92 dBm`: the strength first, the noise after it.
fn signal_noise(text: &str) -> Option<i32> {
    text.split_whitespace().next()?.parse().ok()
}

// The Windows numbering the report reads: open, WPA, WPA2, WPA3.
fn security_of(mode: &str) -> (i32, i32, bool) {
    match () {
        _ if mode.is_empty() || mode.ends_with("none") || mode.ends_with("open") => (1, 0, false),
        _ if mode.contains("wep") => (1, 1, true),
        _ if mode.contains("wpa3") => (9, 4, true),
        _ if mode.contains("wpa2") => (7, 4, true),
        _ if mode.contains("wpa") => (4, 2, true),
        _ => (1, 0, true),
    }
}

// The key is in the keychain, which hands it over only once the person at the
// machine has said so in a window of its own. Asking is the right way round:
// they are the one sharing their network, and nothing here can say yes for
// them. A refusal reads the same as a key that is not there.
pub fn wifi_profile_xml(profile: &str) -> Option<String> {
    const SYSTEM: &str = "/Library/Keychains/System.keychain";
    let security = |keychain: Option<&str>, secret: bool| {
        let mut args = vec!["find-generic-password", "-s", "AirPort", "-a", profile];
        if secret {
            args.push("-w");
        }
        args.extend(keychain);
        command_output("security", &args).map(|out| out.trim().to_string())
    };
    // Whether it is there is answered without a window; only the key itself
    // costs one. The one this person joined is in their keychain, one the
    // machine joined for everybody in the machine's, and the window is put
    // up once, for whichever has it, rather than again for the other after
    // a no.
    let keychain = [None, Some(SYSTEM)].into_iter().find(|kc| security(*kc, false).is_some())?;
    Some(match security(keychain, true).filter(|key| !key.is_empty()) {
        Some(key) => format!("<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>", xml_escape(&key)),
        None => "<sharedKey><keyType>passPhrase</keyType><protected>true</protected><keyMaterial>*</keyMaterial></sharedKey>".to_string(),
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

    // Here `-W` is the wait in milliseconds and the time to live is `-m`,
    // which is not what either of the other two systems call them.
    pub fn echo(&self, ip: Ipv4Addr, ttl: u8, timeout_ms: u32) -> Option<(Echo, Ipv4Addr, u32)> {
        let out = std::process::Command::new("ping")
            .args(["-c", "1", "-n", "-W", &timeout_ms.max(1).to_string(), "-m", &ttl.to_string(), &ip.to_string()])
            .output()
            .ok()?;
        answer(&String::from_utf8_lossy(&out.stdout))
    }
}

// Both the reply and the complaint of a hop on the way are written as bytes
// from an address; only the reply carries a time.
fn answer(text: &str) -> Option<(Echo, Ipv4Addr, u32)> {
    for line in text.lines() {
        let Some(rest) = line.split_once(" bytes from ").map(|(_, r)| r) else { continue };
        let from: Ipv4Addr = rest.split([':', ' ']).next()?.parse().ok()?;
        if rest.contains("Time to live exceeded") {
            return Some((Echo::Expired, from, 0));
        }
        let Some(ms) = rest.split("time=").nth(1).and_then(|t| t.split_whitespace().next()).and_then(|t| t.parse::<f32>().ok()) else {
            return Some((Echo::Failed, from, 0));
        };
        return Some((Echo::Reply, from, ms.round() as u32));
    }
    None
}

pub fn download(url: &str, to: &std::path::Path) -> bool {
    command_output("curl", &["-sSL", "--proto", "=https", "--max-time", "120", "-o", &to.display().to_string(), url]).is_some() && to.is_file()
}

// Nothing to hand a package to here: what put him on the machine takes care
// of replacing him, and it is not his business to do it himself.
pub fn install_package(_path: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ifconfig_gives_the_adapters_that_are_up_and_addressed() {
        let text = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\n\tstatus: active\nen0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 3c:22:fb:01:02:03\n\tinet6 fe80::14b1:7cff:fe00:1%en0 prefixlen 64 secured scopeid 0xc\n\tinet 192.168.1.10 netmask 0xffffff00 broadcast 192.168.1.255\n\tmedia: autoselect (1000baseT <full-duplex>)\n\tstatus: active\nen5: flags=8863<UP> mtu 1500\n\tether 3c:22:fb:01:02:04\n\tstatus: inactive\n";
        let found = parse_ifconfig(text);
        assert_eq!(found.len(), 1, "the loopback and a cable nobody plugged in are neither of them a way out");
        let en0 = &found[0];
        assert_eq!(en0.name, "en0");
        assert_eq!(en0.mac, Some([0x3c, 0x22, 0xfb, 0x01, 0x02, 0x03]));
        assert_eq!(en0.addresses, vec![("fe80::14b1:7cff:fe00:1".parse::<IpAddr>().unwrap(), 64), ("192.168.1.10".parse::<IpAddr>().unwrap(), 24)]);
        assert_eq!(en0.speed, 1_000_000_000);
    }

    #[test]
    fn the_default_route_names_its_way_out() {
        let names = vec!["en0".to_string(), "utun4".to_string()];
        let text = "Routing tables\n\nInternet:\nDestination        Gateway            Flags        Netif Expire\ndefault            192.168.1.1        UGScg          en0\ndefault            link#22            UCSIg        utun4\n192.168.1          link#12            UCS            en0\n";
        assert_eq!(default_routes(text, &names), vec![("en0".to_string(), "192.168.1.1".parse::<IpAddr>().unwrap())]);
    }

    #[test]
    fn the_resolvers_belong_to_the_interfaces_they_are_named_against() {
        let text = "DNS configuration\n\nresolver #1\n  search domain[0] : lan\n  nameserver[0] : 192.168.1.1\n  nameserver[1] : 1.1.1.1\n  if_index : 12 (en0)\n  flags    : Request A records\n\nresolver #2\n  domain   : local\n  options  : mdns\n";
        let found = parse_scutil_dns(text);
        assert_eq!(found, vec![("en0".to_string(), vec!["192.168.1.1".parse::<IpAddr>().unwrap(), "1.1.1.1".parse::<IpAddr>().unwrap()])]);
    }

    #[test]
    fn a_netmask_written_in_hex_counts_as_a_prefix() {
        assert_eq!((prefix_of("0xffffff00"), prefix_of("0xfffffe00"), prefix_of("0xffffffff")), (24, 23, 32));
        assert_eq!(media_speed("media: autoselect (1000baseT <full-duplex>)"), 1_000_000_000);
        assert_eq!(media_speed("media: autoselect"), 0, "a wireless card names no speed here");
    }

    #[test]
    fn the_channel_says_where_in_the_spectrum_it_sits() {
        assert_eq!(channel_of("149 (5GHz, 80MHz)"), (Some(149), Some(5745)));
        assert_eq!(channel_of("6 (2GHz, 20MHz)"), (Some(6), Some(2437)));
        assert_eq!(channel_of("37 (6GHz, 160MHz)"), (Some(37), Some(6135)));
        assert_eq!(channel_of(""), (None, None));
        assert_eq!(signal_noise("-45 dBm / -92 dBm"), Some(-45));
    }

    #[test]
    fn the_security_mode_reads_as_the_report_numbers_it() {
        assert_eq!(security_of("spairport_security_mode_wpa3_personal"), (9, 4, true));
        assert_eq!(security_of("spairport_security_mode_wpa2_personal"), (7, 4, true));
        assert_eq!(security_of("spairport_security_mode_none"), (1, 0, false));
    }

    #[test]
    fn a_ping_is_read_by_what_answered_it() {
        let reply = "64 bytes from 1.1.1.1: icmp_seq=0 ttl=57 time=12.345 ms";
        assert_eq!(answer(reply), Some((Echo::Reply, Ipv4Addr::new(1, 1, 1, 1), 12)));
        let expired = "92 bytes from 192.168.1.1: Time to live exceeded\nVr HL TOS  Len   ID Flg  off TTL Pro  cks      Src      Dst";
        assert_eq!(answer(expired), Some((Echo::Expired, Ipv4Addr::new(192, 168, 1, 1), 0)));
        assert_eq!(answer("Request timeout for icmp_seq 0"), None);
    }

    #[test]
    #[ignore]
    fn what_this_machine_is_connected_through() {
        for a in adapters() {
            println!("{} mac={:?} dhcp={} speed={} gw={:?} dns={:?}", a.name, a.mac, a.dhcp, a.speed, a.gateways, a.dns);
            println!("  addresses {:?}", a.addresses);
        }
        match wifi() {
            WifiState::Denied => println!("wifi: the name is kept back until this Mac is given location access"),
            WifiState::Connected(w) => println!(
                "wifi: ssid={} signal={} rssi={:?} ch={:?} freq={:?} rate={} auth={} cipher={} secured={}",
                w.ssid, w.signal, w.rssi, w.channel, w.frequency, w.rx_mbps, w.auth, w.cipher, w.secured
            ),
            _ => println!("wifi: no network"),
        }
        println!("key kept under lock: {:?}", wifi_profile_xml("whatever").is_some());
    }
}
