use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::detect::detect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Service {
    MsTelemetry,
    WindowsUpdate,
    Microsoft,
    OneDrive,
    Teams,
    Google,
    YouTube,
    Trackers,
    GitHub,
    Claude,
    OpenAi,
    Cloudflare,
    Aws,
    Akamai,
    Steam,
    Discord,
    Spotify,
    Netflix,
    Signal,
    WhatsApp,
    Telegram,
    Slack,
    Zoom,
    Meta,
    TikTok,
    Apple,
    NordVpn,
    Tailscale,
    Tor,
    Seznam,
    RustCrates,
    Mozilla,
    Proton,
    Mullvad,
    Dropbox,
    GoogleDrive,
    Gemini,
    Copilot,
    Perplexity,
    Mistral,
    DeepSeek,
    Grok,
    HuggingFace,
    Ollama,
    Twitch,
    Reddit,
    Twitter,
    Bluesky,
    LinkedIn,
    EpicGames,
    BattleNet,
    Riot,
    Xbox,
    Ea,
    Ubisoft,
    Gog,
    Wargaming,
    Adobe,
    JetBrains,
    VsCode,
    Npm,
    PyPi,
    DockerHub,
    AnyDesk,
    TeamViewer,
    RustDesk,
    Nvidia,
    Antivirus,
    Plex,
    Syncthing,
    Torrent,
    Wikipedia,
    CzechNews,
    Alza,
    ChinaShops,
    Yandex,
    Vk,
    DuckDuckGo,
    Viber,
    Matrix,
    Webex,
    GoogleMeet,
    Xiaomi,
    Tuya,
    Hue,
    Sonos,
    HomeAssistant,
    Mega,
    WeTransfer,
    UlozTo,
    FoodDelivery,
    Private,
}

impl Service {
    pub fn discreet(self) -> bool {
        self == Service::Private
    }
}

// Longest suffix first, so the most specific match wins.
fn domains() -> &'static [(String, Service)] {
    static DOMAINS: OnceLock<Vec<(String, Service)>> = OnceLock::new();
    DOMAINS.get_or_init(|| {
        let mut all: Vec<(String, Service)> =
            detect().service.iter().flat_map(|known| known.domains.iter().map(move |d| (d.to_lowercase(), known.name))).collect();
        all.sort_by_key(|(domain, _)| std::cmp::Reverse(domain.len()));
        all
    })
}

fn processes() -> &'static HashMap<String, Service> {
    static PROCESSES: OnceLock<HashMap<String, Service>> = OnceLock::new();
    PROCESSES.get_or_init(|| {
        detect().service.iter().flat_map(|known| known.processes.iter().map(move |p| (p.to_lowercase(), known.name))).collect()
    })
}

fn under(name: &str, suffix: &str) -> bool {
    name == suffix || (name.len() > suffix.len() && name.ends_with(suffix) && name.as_bytes()[name.len() - suffix.len() - 1] == b'.')
}

#[cfg(test)]
pub fn known() -> Vec<Service> {
    let mut all: Vec<Service> = detect().service.iter().map(|known| known.name).collect();
    all.sort_by_key(|s| format!("{s:?}"));
    all.dedup();
    all
}

pub fn calls_home(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_lowercase();
    matches!(by_domain(&name), Some(Service::MsTelemetry | Service::Trackers))
        || detect().calls_home_words.iter().any(|w| name.contains(w.as_str()))
}

pub fn by_domain(name: &str) -> Option<Service> {
    let name = name.trim_end_matches('.').to_lowercase();
    if detect().private_words.iter().any(|w| name.contains(w.as_str())) {
        return Some(Service::Private);
    }
    domains().iter().find(|(suffix, _)| under(&name, suffix)).map(|&(_, s)| s)
}

// The table is lowercase; Linux process names are not, and the lookup does not
// depend on the caller having lowercased the name first.
pub fn by_process(process: &str) -> Option<Service> {
    processes().get(&process.to_lowercase()).copied()
}

pub fn identify(process: &str, name: Option<&str>) -> Option<Service> {
    name.and_then(by_domain).or_else(|| by_process(process))
}

pub fn plain_http_by_design(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_lowercase();
    let plain = &detect().plain_http;
    plain.labels.iter().any(|label| name.starts_with(label.as_str())) || plain.suffixes.iter().any(|s| under(&name, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocation_checks_are_plain_by_design() {
        assert!(plain_http_by_design("ocsp.digicert.com"));
        assert!(plain_http_by_design("crl.microsoft.com."));
        assert!(plain_http_by_design("x1.c.lencr.org"));
        assert!(!plain_http_by_design("example.com"));
        assert!(!plain_http_by_design("notmicrosoft.com"));
    }

    #[test]
    fn domains_match_on_label_boundaries() {
        assert_eq!(by_domain("v10.events.data.microsoft.com"), Some(Service::MsTelemetry));
        assert!(calls_home("v10.events.data.microsoft.com"));
        assert_eq!(by_domain("gemini.google.com"), Some(Service::Gemini));
        assert_eq!(by_domain("www.google.com"), Some(Service::Google));
        assert_eq!(by_domain("api.x.com"), Some(Service::Twitter));
        assert_eq!(by_domain("box.com"), None);
        assert_eq!(by_process("anydesk"), Some(Service::AnyDesk));
        assert!(calls_home("o4501.ingest.sentry.io."));
        assert!(!calls_home("github.com"));
        assert_eq!(by_domain("www.microsoft.com"), Some(Service::Microsoft));
        assert_eq!(by_domain("rr3---sn-2gb7sn7k.googlevideo.com"), Some(Service::YouTube));
        assert_eq!(by_domain("notgithub.com"), None);
        assert_eq!(by_domain("GitHub.com."), Some(Service::GitHub));
    }

    #[test]
    fn the_longest_domain_wins() {
        assert_eq!(by_domain("docs.google.com"), Some(Service::GoogleDrive));
        assert_eq!(by_domain("teams.microsoft.com"), Some(Service::Teams));
        assert_eq!(by_domain("onedrive.live.com"), Some(Service::OneDrive));
        assert_eq!(by_domain("update.microsoft.com"), Some(Service::WindowsUpdate));
        assert_eq!(by_domain("copilot.microsoft.com"), Some(Service::Copilot));
        assert_eq!(by_domain("live.com"), Some(Service::Microsoft));
    }

    #[test]
    fn private_names_are_recognised() {
        assert_eq!(by_domain("ib.fio.cz"), Some(Service::Private));
        assert_eq!(by_domain("online.mojebanka.cz"), Some(Service::Private));
        assert!(Service::Private.discreet());
    }

    #[test]
    fn a_process_name_matches_whatever_its_capitals() {
        assert_eq!(by_process("AnyDesk"), Some(Service::AnyDesk));
        assert_eq!(by_process("Signal"), Some(Service::Signal));
        assert_eq!(identify("Steam", Some("unknown.example")), Some(Service::Steam));
    }

    #[test]
    fn process_is_the_fallback() {
        assert_eq!(identify("signal", None), Some(Service::Signal));
        assert_eq!(identify("msedge", None), None);
        assert_eq!(identify("msedge", Some("www.youtube.com")), Some(Service::YouTube));
        assert_eq!(identify("steam", Some("unknown.example")), Some(Service::Steam));
    }
}
