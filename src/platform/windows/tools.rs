use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_BUFFER_OVERFLOW, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, ICMP_ECHO_REPLY,
    IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH, IP_OPTION_INFORMATION, IP_SUCCESS,
    IP_TTL_EXPIRED_TRANSIT, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2, SendARP,
};
use windows_sys::Win32::NetworkManagement::WiFi::{
    WLAN_BSS_LIST, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
    WLAN_PROFILE_GET_PLAINTEXT_KEY, WlanFreeMemory, WlanGetNetworkBssList, WlanGetProfile, WlanOpenHandle, WlanQueryInterface, dot11_BSS_type_any,
    wlan_interface_state_connected, wlan_intf_opcode_channel_number, wlan_intf_opcode_current_connection,
};
use windows_sys::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKET_ADDRESS};
use windows_sys::core::GUID;

use super::system::CREATE_NO_WINDOW;
use crate::tools::{Adapter, Echo, Wifi, WifiState};

// IfOperStatusUp, which windows-sys does not export under this name.
const IF_OPER_STATUS_UP: i32 = 1;

unsafe fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

fn wide_array(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}

unsafe fn ip_of(address: &SOCKET_ADDRESS) -> Option<IpAddr> {
    if address.lpSockaddr.is_null() {
        return None;
    }
    let p = address.lpSockaddr as *const u8;
    unsafe {
        match u16::from_ne_bytes([*p, *p.add(1)]) {
            AF_INET => {
                let b = std::slice::from_raw_parts(p.add(4), 4);
                Some(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3])))
            }
            AF_INET6 => {
                let b: [u8; 16] = std::slice::from_raw_parts(p.add(8), 16).try_into().ok()?;
                Some(IpAddr::V6(Ipv6Addr::from(b)))
            }
            _ => None,
        }
    }
}

pub fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size: u32 = 32 * 1024;
    // u64 words keep the buffer aligned for the structures written into it.
    let mut buf: Vec<u64>;
    loop {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        let rc = unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buf.as_mut_ptr().cast(), &mut size) };
        match rc {
            0 => break,
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return Vec::new(),
        }
    }
    let mut out = Vec::new();
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        let a = unsafe { &*p };
        p = a.Next;
        if a.OperStatus != IF_OPER_STATUS_UP || a.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        let mut addresses = Vec::new();
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let x = unsafe { &*u };
            if let Some(ip) = unsafe { ip_of(&x.Address) } {
                addresses.push((ip, x.OnLinkPrefixLength));
            }
            u = x.Next;
        }
        let mut gateways = Vec::new();
        let mut g = a.FirstGatewayAddress;
        while !g.is_null() {
            let x = unsafe { &*g };
            gateways.extend(unsafe { ip_of(&x.Address) });
            g = x.Next;
        }
        let mut dns = Vec::new();
        let mut d = a.FirstDnsServerAddress;
        while !d.is_null() {
            let x = unsafe { &*d };
            dns.extend(unsafe { ip_of(&x.Address) });
            d = x.Next;
        }
        out.push(Adapter {
            name: unsafe { wide_str(a.FriendlyName) },
            description: unsafe { wide_str(a.Description) },
            mac: (a.PhysicalAddressLength == 6).then(|| a.PhysicalAddress[..6].try_into().unwrap()),
            addresses,
            gateways,
            dns,
            // IP_ADAPTER_DHCP_ENABLED
            dhcp: unsafe { a.Anonymous2.Flags } & 0x4 != 0,
            speed: a.TransmitLinkSpeed.max(a.ReceiveLinkSpeed),
        });
    }
    out.sort_by_key(|a| !a.gateways.iter().any(|g| !g.is_unspecified()));
    out
}


// A whole address, `https://host/path`: the scheme is always that one and is
// not read, since nothing here is fetched over anything else.
pub fn http_get(url: &str) -> Option<String> {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host_part, path_part) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let (agent, host, verb, path) = (wide("raccy"), wide(host_part), wide("GET"), wide(&path_part));
    unsafe {
        let session = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0);
        if session.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session, 4000, 4000, 4000, 4000);
        let connect = WinHttpConnect(session, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0);
        let request = match connect.is_null() {
            true => std::ptr::null_mut(),
            false => WinHttpOpenRequest(
                connect,
                verb.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
        };
        let mut body = Vec::new();
        let sent = !request.is_null()
            && WinHttpSendRequest(request, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) != 0
            && WinHttpReceiveResponse(request, std::ptr::null_mut()) != 0;
        if sent {
            let mut chunk = [0u8; 256];
            loop {
                let mut read = 0u32;
                let ok = WinHttpReadData(request, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read) != 0;
                if !ok || read == 0 || body.len() > 1024 {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
            }
        }
        for handle in [request, connect, session] {
            if !handle.is_null() {
                WinHttpCloseHandle(handle);
            }
        }
        (!body.is_empty()).then(|| String::from_utf8_lossy(&body).trim().to_string())
    }
}

pub fn wifi() -> WifiState {
    unsafe {
        let (mut version, mut handle) = (0u32, std::ptr::null_mut());
        if WlanOpenHandle(2, std::ptr::null(), &mut version, &mut handle) != 0 {
            return WifiState::NoAdapter;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let state = if WlanEnumInterfaces(handle, std::ptr::null(), &mut list) != 0 || list.is_null() {
            WifiState::NoAdapter
        } else {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            let state = match items.iter().find(|i| i.isState == wlan_interface_state_connected) {
                _ if n == 0 => WifiState::NoAdapter,
                None => WifiState::Disconnected,
                Some(info) => connection(handle, &info.InterfaceGuid),
            };
            WlanFreeMemory(list.cast());
            state
        };
        WlanCloseHandle(handle, std::ptr::null());
        state
    }
}

unsafe fn connection(handle: HANDLE, guid: &GUID) -> WifiState {
    unsafe {
        let (mut size, mut data) = (0u32, std::ptr::null_mut());
        let rc = WlanQueryInterface(
            handle,
            guid,
            wlan_intf_opcode_current_connection,
            std::ptr::null(),
            &mut size,
            &mut data,
            std::ptr::null_mut(),
        );
        if rc == ERROR_ACCESS_DENIED {
            return WifiState::Denied;
        }
        if rc != 0 || data.is_null() {
            return WifiState::Disconnected;
        }
        let attributes = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
        let association = &attributes.wlanAssociationAttributes;
        let raw_ssid = association.dot11Ssid;
        let security = &attributes.wlanSecurityAttributes;
        let mut wifi = Wifi {
            ssid: String::from_utf8_lossy(&raw_ssid.ucSSID[..(raw_ssid.uSSIDLength as usize).min(32)]).into_owned(),
            profile: wide_array(&attributes.strProfileName),
            bssid: association.dot11Bssid,
            signal: association.wlanSignalQuality,
            rssi: None,
            frequency: None,
            channel: None,
            rx_mbps: association.ulRxRate / 1000,
            tx_mbps: association.ulTxRate / 1000,
            auth: security.dot11AuthAlgorithm,
            cipher: security.dot11CipherAlgorithm,
            secured: security.bSecurityEnabled != 0,
        };
        WlanFreeMemory(data.cast());

        let (mut size, mut data) = (0u32, std::ptr::null_mut());
        let rc = WlanQueryInterface(
            handle,
            guid,
            wlan_intf_opcode_channel_number,
            std::ptr::null(),
            &mut size,
            &mut data,
            std::ptr::null_mut(),
        );
        if rc == 0 && !data.is_null() {
            wifi.channel = Some(*(data as *const u32));
            WlanFreeMemory(data.cast());
        }

        let mut bss: *mut WLAN_BSS_LIST = std::ptr::null_mut();
        let rc = WlanGetNetworkBssList(handle, guid, &raw_ssid, dot11_BSS_type_any, wifi.secured as i32, std::ptr::null(), &mut bss);
        if rc == 0 && !bss.is_null() {
            let entries = std::slice::from_raw_parts((*bss).wlanBssEntries.as_ptr(), (*bss).dwNumberOfItems as usize);
            if let Some(entry) = entries.iter().find(|e| e.dot11Bssid == wifi.bssid) {
                wifi.rssi = Some(entry.lRssi);
                wifi.frequency = Some(entry.ulChCenterFrequency / 1000);
            }
            WlanFreeMemory(bss.cast());
        }
        WifiState::Connected(wifi)
    }
}

pub fn wifi_profile_xml(profile: &str) -> Option<String> {
    let name: Vec<u16> = profile.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let (mut version, mut handle) = (0u32, std::ptr::null_mut());
        if WlanOpenHandle(2, std::ptr::null(), &mut version, &mut handle) != 0 {
            return None;
        }
        let mut found = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(handle, std::ptr::null(), &mut list) == 0 && !list.is_null() {
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            if let Some(info) = items.iter().find(|i| i.isState == wlan_interface_state_connected) {
                let (mut xml, mut flags, mut access) = (std::ptr::null_mut(), WLAN_PROFILE_GET_PLAINTEXT_KEY, 0u32);
                let rc = WlanGetProfile(handle, &info.InterfaceGuid, name.as_ptr(), std::ptr::null(), &mut xml, &mut flags, &mut access);
                if rc == 0 && !xml.is_null() {
                    found = Some(wide_str(xml));
                    WlanFreeMemory(xml.cast());
                }
            }
            WlanFreeMemory(list.cast());
        }
        WlanCloseHandle(handle, std::ptr::null());
        found
    }
}

pub fn arp(ip: Ipv4Addr, source: Ipv4Addr) -> Option<[u8; 6]> {
    let mut mac = [0u8; 8];
    let mut len = mac.len() as u32;
    let rc = unsafe {
        SendARP(u32::from_ne_bytes(ip.octets()), u32::from_ne_bytes(source.octets()), mac.as_mut_ptr().cast(), &mut len)
    };
    (rc == 0 && len == 6).then(|| mac[..6].try_into().unwrap())
}

pub struct Pinger(HANDLE);

impl Pinger {
    pub fn open() -> Option<Pinger> {
        let icmp = unsafe { IcmpCreateFile() };
        (icmp != INVALID_HANDLE_VALUE).then_some(Pinger(icmp))
    }

    pub fn echo(&self, ip: Ipv4Addr, ttl: u8, timeout_ms: u32) -> Option<(Echo, Ipv4Addr, u32)> {
        let payload = [0x52u8; 32];
        let mut buf = vec![0u8; size_of::<ICMP_ECHO_REPLY>() + payload.len() + 64];
        let options = IP_OPTION_INFORMATION { Ttl: ttl, Tos: 0, Flags: 0, OptionsSize: 0, OptionsData: std::ptr::null_mut() };
        unsafe {
            IcmpSendEcho2(
                self.0,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                u32::from_ne_bytes(ip.octets()),
                payload.as_ptr().cast(),
                payload.len() as u16,
                &options,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                timeout_ms,
            );
        }
        // A hop whose time ran out still fills in the reply, so read it either way.
        let reply: ICMP_ECHO_REPLY = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        if reply.Address == 0 {
            return None;
        }
        let echo = match reply.Status {
            IP_SUCCESS => Echo::Reply,
            // IP_TTL_EXPIRED_TRANSIT and IP_TTL_EXPIRED_REASSEM
            IP_TTL_EXPIRED_TRANSIT | 11014 => Echo::Expired,
            _ => Echo::Failed,
        };
        Some((echo, Ipv4Addr::from(reply.Address.to_ne_bytes()), reply.RoundTripTime))
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        unsafe { IcmpCloseHandle(self.0) };
    }
}

// A file from an address, straight to disk. Windows ship curl, and a file of
// megabytes does not belong in the in-process fetch above.
pub fn download(url: &str, to: &std::path::Path) -> bool {
    // Windows' own curl, by its full path rather than by whatever is first on
    // the way; https on the redirects too, or a hop could take it to plain.
    let child = Command::new(system32("curl.exe"))
        .args(["-sSL", "--proto", "=https", "--proto-redir", "=https", "--max-time", "120", "-o", &to.display().to_string(), url])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    super::as_helper(&mut child, |child| child.wait()).is_ok_and(|s| s.success()) && to.is_file()
}

// Hands the package to Windows Installer, which asks for administrator
// itself. The package stops the running Raccy and starts the new one.
// The package stops the Raccy that started it, so there is nobody left here
// to hear how it went: it is handed over and let go of. Failing to start it
// at all is the one answer that comes back.
pub fn install_package(path: &std::path::Path) -> bool {
    match Command::new(system32("msiexec.exe")).args(["/i", &path.display().to_string(), "/qb"]).spawn() {
        Ok(child) => {
            super::keep_as_helper(child.id());
            true
        }
        Err(_) => false,
    }
}

// A program of Windows' own, by its full path.
fn system32(program: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from);
    root.unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")).join("System32").join(program)
}
