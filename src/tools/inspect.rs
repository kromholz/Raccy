use std::fs::File;
use std::path::Path;

use crate::lang::Lang;
use crate::platform;
use crate::tools::{Report, row};

// The mark of the web: zone 3 is the internet, 4 the restricted sites.
pub const ZONE_INTERNET: u32 = 3;

#[derive(Debug, PartialEq)]
pub enum Dropped {
    Folder(String),
    Unreadable(String),
    InCloud(String),
    File(Look),
}

#[derive(Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
    pub signature: Signature,
    pub origin: Option<Origin>,
}

// Everything but NotSignable is for a system that signs its programs,
// which here is Windows; a Linux build never makes one of them.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub enum Signature {
    Valid { signer: String, catalog: bool },
    Broken { signer: Option<String>, why: Broken },
    Unsigned,
    Unchecked,
    NotSignable,
}

// Nothing on Linux looks, so none of these is ever made there.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Broken {
    Tampered,
    Distrusted,
    Expired,
    UntrustedRoot,
    Other,
}

#[derive(Debug, PartialEq)]
pub struct Origin {
    pub zone: u32,
    pub host: Option<String>,
}

pub fn report(path: &Path, more: usize, lang: Lang) -> Report {
    let dropped = look(path);
    let salt = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut text = String::new();
    match &dropped {
        Dropped::Folder(name) | Dropped::Unreadable(name) | Dropped::InCloud(name) => row(&mut text, lang.inspect_label(0), name),
        Dropped::File(l) => {
            row(&mut text, lang.inspect_label(0), &l.name);
            row(&mut text, lang.inspect_label(1), &size_text(l.size));
            row(&mut text, "sha-256", &l.sha256);
            row(&mut text, lang.inspect_label(2), &lang.signature_text(&l.signature));
            row(&mut text, lang.inspect_label(3), &lang.origin_text(l.origin.as_ref()));
        }
    }
    if more > 0 {
        text.push('\n');
        text.push_str(&lang.inspect_more(more));
        text.push('\n');
    }
    Report { title: lang.inspect_title().into(), text, line: lang.dropped_line(&dropped, salt) }
}

pub fn look(path: &Path) -> Dropped {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    if path.is_dir() {
        return Dropped::Folder(name);
    }
    if platform::inspect::in_cloud(path) {
        return Dropped::InCloud(name);
    }
    let Ok(mut file) = File::open(path) else { return Dropped::Unreadable(name) };
    let size = file.metadata().map_or(0, |m| m.len());
    let Some((sha256, head)) = platform::inspect::sha256(&mut file) else { return Dropped::Unreadable(name) };
    let executable = head.starts_with(b"MZ") || head.starts_with(b"\x7fELF") || head.starts_with(b"#!");
    // A program too broken for its signature format still has no signature.
    let signature = match platform::inspect::signature(path, size) {
        Signature::NotSignable if executable => Signature::Unsigned,
        signature => signature,
    };
    Dropped::File(Look {
        name,
        size,
        sha256,
        executable,
        signature,
        origin: platform::inspect::origin(path),
    })
}

fn size_text(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") } else { crate::talk::size(bytes) }
}

pub(crate) fn parse_zone(text: &str) -> Option<Origin> {
    let value = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('=').map(str::trim));
    let zone = value("ZoneId")?.parse().ok()?;
    let host = value("HostUrl").and_then(host_of).or_else(|| value("ReferrerUrl").and_then(host_of));
    Some(Origin { zone, host })
}

// Only the host of a download address: the rest can carry tokens.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

// SHA-256 of a file, in Rust, for every system that has no such thing of its
// own to call. Windows does, and calls that instead.
#[cfg(not(windows))]
pub(crate) fn sha256_of(file: &mut impl std::io::Read) -> Option<(String, Vec<u8>)> {
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        if head.len() < 4 {
            head.extend_from_slice(&buf[..n.min(4 - head.len())]);
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finish();
    Some((digest.iter().map(|b| format!("{b:02x}")).collect(), head))
}

// SHA-256, as FIPS 180-4 has it.
#[cfg(not(windows))]
struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    length: u64,
}

#[cfg(not(windows))]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[cfg(not(windows))]
impl Sha256 {
    fn new() -> Sha256 {
        Sha256 {
            state: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buffer: Vec::with_capacity(64),
            length: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        if !self.buffer.is_empty() {
            let take = (64 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == 64 {
                let block: [u8; 64] = self.buffer[..].try_into().unwrap_or([0; 64]);
                self.block(&block);
                self.buffer.clear();
            }
        }
        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            self.block(chunk.try_into().unwrap_or(&[0; 64]));
        }
        self.buffer.extend_from_slice(chunks.remainder());
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.length * 8;
        let mut tail = self.buffer.clone();
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        self.buffer.clear();
        for chunk in tail.chunks_exact(64) {
            self.block(chunk.try_into().unwrap_or(&[0; 64]));
        }
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn block(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Whatever each system hashes with, it must answer the same as FIPS 180-4.
    #[test]
    fn sha256_matches_the_known_answers() {
        let hash = |text: &str| platform::inspect::sha256(&mut text.as_bytes()).map(|(h, _)| h);
        assert_eq!(hash("abc").as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(hash("").as_deref(), Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
        let long = "a".repeat(1_000_000);
        assert_eq!(hash(&long).as_deref(), Some("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
    }

    #[test]
    fn the_mark_of_the_web_names_only_the_host() {
        let text = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://github.com/x/y/releases\r\nHostUrl=https://user:pw@objects.githubusercontent.com/abc?token=secret\r\n";
        assert_eq!(parse_zone(text), Some(Origin { zone: 3, host: Some("objects.githubusercontent.com".into()) }));
        let local = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n";
        assert_eq!(parse_zone(local), Some(Origin { zone: 3, host: None }));
        assert_eq!(parse_zone("garbage"), None);
    }

    #[test]
    fn the_hash_is_sha_256() {
        let path = std::env::temp_dir().join(format!("raccy-hash-{}.txt", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let look = look(&path);
        std::fs::remove_file(&path).ok();
        let Dropped::File(look) = look else { panic!("{look:?}") };
        assert_eq!(look.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(!look.executable);
        assert_eq!(look.size, 3);
    }

    #[test]
    fn a_folder_is_not_a_file() {
        assert!(matches!(look(&std::env::temp_dir()), Dropped::Folder(_)));
    }

    #[test]
    fn an_unsigned_file_is_not_signed_by_anyone() {
        let path = std::env::temp_dir().join(format!("raccy-unsigned-{}.exe", std::process::id()));
        std::fs::write(&path, b"MZ not really").unwrap();
        let signed = platform::inspect::signed_by(&path, &"0".repeat(64));
        std::fs::remove_file(&path).ok();
        assert!(!signed);
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn our_own_signature_pins_live() {
        let signed = Path::new(env!("CARGO_MANIFEST_DIR")).join(r"target\release\raccy.exe");
        let pin = std::env::var("RACCY_SIGNER").expect("the name, as build.ps1 sets it");
        assert!(platform::inspect::signed_by(&signed, &pin), "the signed build");
        assert!(!platform::inspect::signed_by(&signed, "CN=Somebody Else"), "another name");
        let mut bytes = std::fs::read(&signed).unwrap();
        let middle = bytes.len() / 3;
        bytes[middle] ^= 1;
        let tampered = std::env::temp_dir().join(format!("raccy-tampered-{}.exe", std::process::id()));
        std::fs::write(&tampered, bytes).unwrap();
        let ok = platform::inspect::signed_by(&tampered, &pin);
        std::fs::remove_file(&tampered).ok();
        assert!(!ok, "a changed byte");
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(!platform::inspect::signed_by(&Path::new(&windows).join(r"System32\notepad.exe"), &pin), "notepad");
    }

    #[test]
    #[ignore]
    #[cfg(windows)]
    fn windows_files_are_signed_live() {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for file in [r"System32\notepad.exe", r"System32\kernel32.dll", r"System32\drivers\etc\hosts"] {
            let Dropped::File(look) = look(&Path::new(&windows).join(file)) else { panic!("{file}") };
            println!("{file}: {:?}", look.signature);
        }
    }
}

