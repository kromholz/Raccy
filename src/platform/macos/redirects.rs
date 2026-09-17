use super::command_output;
use crate::watch::redirects::{Look, parse_hosts};

pub fn look() -> Look {
    let hosts = std::fs::read_to_string("/etc/hosts").ok().map(|text| parse_hosts(&text));
    Look { hosts, proxy: Some(proxy()), dns: crate::tools::dns_by_network() }
}

// A proxy set for the whole machine, as the network settings keep it, or one
// set for programs started from a shell.
fn proxy() -> String {
    if let Some(text) = command_output("scutil", &["--proxy"])
        && let Some(host) = proxy_host(&text)
    {
        return host;
    }
    ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
}

// The dictionary scutil prints: HTTPSEnable 1 and HTTPSProxy beside it.
fn proxy_host(text: &str) -> Option<String> {
    let value = |name: &str| text.lines().find_map(|l| l.trim().strip_prefix(name)?.trim().strip_prefix(':').map(str::trim).map(str::to_string));
    for (on, host, port) in [("HTTPSEnable", "HTTPSProxy", "HTTPSPort"), ("HTTPEnable", "HTTPProxy", "HTTPPort")] {
        if value(on).as_deref() == Some("1")
            && let Some(host) = value(host)
        {
            return Some(match value(port) {
                Some(port) => format!("{host}:{port}"),
                None => host,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_is_read_from_the_settings_as_they_are_printed() {
        let text = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 1\n  HTTPSProxy : proxy.example\n  HTTPSPort : 3128\n}";
        assert_eq!(proxy_host(text).as_deref(), Some("proxy.example:3128"));
        assert_eq!(proxy_host("<dictionary> {\n  HTTPSEnable : 0\n}"), None, "set but switched off");
    }
}
