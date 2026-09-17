use super::command_output;
use crate::watch::redirects::{Look, parse_hosts};

pub fn look() -> Look {
    let hosts = std::fs::read_to_string("/etc/hosts").ok().map(|text| parse_hosts(&text));
    Look { hosts, proxy: Some(proxy()), dns: crate::tools::dns_by_network() }
}

fn proxy() -> String {
    ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
}

// Whether resolvectl is there to ask, for the nameservers behind a stub.
pub(super) fn resolvectl_dns(interface: &str) -> Option<Vec<String>> {
    let out = command_output("resolvectl", &["dns", interface])?;
    let servers: Vec<String> = out.split_once(':')?.1.split_whitespace().map(|s| s.to_string()).collect();
    (!servers.is_empty()).then_some(servers)
}
