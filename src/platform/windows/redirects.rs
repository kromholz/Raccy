use std::path::PathBuf;

use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

use super::system::RegKey;
use crate::watch::redirects::{Look, parse_hosts};

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

pub fn look() -> Look {
    let hosts = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join(r"System32\drivers\etc\hosts"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_hosts(&text));
    Look { hosts, proxy: proxy(), dns: crate::tools::dns_by_network() }
}

fn proxy() -> Option<String> {
    let key = RegKey::open(HKEY_CURRENT_USER, INTERNET_SETTINGS)?;
    let server = (key.dword("ProxyEnable") == Some(1)).then(|| key.string("ProxyServer")).flatten().filter(|s| !s.is_empty());
    Some(server.or_else(|| key.string("AutoConfigURL").filter(|s| !s.is_empty())).unwrap_or_default())
}
