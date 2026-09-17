use std::path::PathBuf;

use crate::platform;

// Where a new Raccy is announced. A file with nothing in the request: no
// query, no identifier, no version of his own sent anywhere, so that what the
// README says about sending nothing about you stays true.
const FEED: &str = "https://updates.bohemia.systems/raccy/stable.json";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

// A version as three numbers. Anything that does not read that way is no
// version and is never newer than what is running.
fn numbers(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.trim().split('.').map(|p| p.trim().parse::<u32>());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c)), None) => Some((a, b, c)),
        _ => None,
    }
}

pub fn newer(theirs: &str, ours: &str) -> bool {
    match (numbers(theirs), numbers(ours)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => false,
    }
}

// What the feed says, taking only what is needed and only where it is whole.
// The address must be on the same host the feed came from: a file that says
// to fetch from somewhere else is not to be believed.
fn parse_feed(json: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let release = Release {
        version: value["version"].as_str()?.trim().to_string(),
        url: value["url"].as_str()?.trim().to_string(),
        sha256: value["sha256"].as_str()?.trim().to_lowercase(),
    };
    let host = |url: &str| url.split_once("://").map(|(_, rest)| rest.split('/').next().unwrap_or("").to_string());
    let same_host = host(&release.url) == host(FEED) && release.url.starts_with("https://");
    let whole = numbers(&release.version).is_some() && release.sha256.len() == 64 && release.sha256.chars().all(|c| c.is_ascii_hexdigit());
    (same_host && whole).then_some(release)
}

// The release waiting out there, if there is one newer than this Raccy.
pub fn look() -> Option<Release> {
    let release = parse_feed(&platform::tools::http_get(FEED)?)?;
    newer(&release.version, env!("CARGO_PKG_VERSION")).then_some(release)
}

// Takes the package and hands it to the system, having made sure twice that
// it is what the feed promised: the hash the feed gave, and the signature of
// whoever builds Raccy. A file that fails either is deleted unopened.
pub fn take(release: &Release) -> bool {
    // A folder of its own each time, named after this run: nothing else can
    // have laid a file there in advance, and no second attempt writes into
    // the one being checked.
    let (a, b) = (std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let dir: PathBuf = std::env::temp_dir().join(format!("raccy-update-{a}-{b:x}"));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let to = dir.join("raccy.msi");
    if !platform::tools::download(&release.url, &to) {
        crate::trace::record(|| format!("update: {} did not come down", release.url));
        let _ = std::fs::remove_dir_all(&dir);
        return false;
    }
    let ours = std::fs::File::open(&to).ok().and_then(|mut file| platform::inspect::sha256(&mut file)).map(|(hash, _)| hash);
    // A build with no certificate built into it takes nothing: better no
    // update at all than one nobody vouched for.
    let signed = option_env!("RACCY_SIGNER").is_some_and(|pin| platform::inspect::signed_by(&to, pin));
    if ours.as_deref() != Some(release.sha256.as_str()) || !signed {
        crate::trace::record(|| format!("update: {} is not what it should be, hash={ours:?} signed={signed}", to.display()));
        let _ = std::fs::remove_dir_all(&dir);
        // Offering it again would only fetch the same bad package.
        *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return false;
    }
    crate::trace::record(|| format!("update: handing over {}", to.display()));
    platform::tools::install_package(&to)
}

// The one found and not yet taken, if any.
static WAITING: std::sync::Mutex<Option<Release>> = std::sync::Mutex::new(None);
// One attempt at a time, and a word owed to whoever asked for it.
static TAKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn waiting() -> Option<Release> {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// Fetches and hands over the package on a thread of its own, once at a time.
// Two goes at the same file would tread on each other, and the answer comes
// back through `failed`, since a thread of its own cannot reach the app.
pub fn take_in_the_background(release: Release) {
    if TAKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("raccy-update-take".into()).spawn(move || {
        let done = take(&release);
        FAILED.store(!done, std::sync::atomic::Ordering::SeqCst);
        TAKING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

// Whether the last attempt came to nothing, asked once and then forgotten.
pub fn failed() -> bool {
    FAILED.swap(false, std::sync::atomic::Ordering::SeqCst)
}

// Looks a few minutes after he starts, and then once a day. Only where there
// is a package to hand over: elsewhere whatever put him on the machine
// replaces him, and going looking is not his business.
pub fn watch() {
    if !cfg!(windows) {
        return;
    }
    let _ = std::thread::Builder::new().name("raccy-update".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(FIRST_LOOK_SECS));
        loop {
            if let Some(release) = look() {
                crate::trace::record(|| format!("update: {} is out", release.version));
                *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = Some(release);
            }
            std::thread::sleep(std::time::Duration::from_secs(LOOK_EVERY_SECS));
        }
    });
}

const FIRST_LOOK_SECS: u64 = 300;
const LOOK_EVERY_SECS: u64 = 24 * 3600;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_numbers_and_newer_or_it_is_nothing() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(newer("0.1.10", "0.1.9"), "ten is after nine, not before it");
        assert!(!newer("0.1.0", "0.1.0"), "the same is not newer");
        assert!(!newer("0.0.9", "0.1.0"), "older is not newer");
        for nonsense in ["", "latest", "0.1", "0.1.0.1", "0.1.x", "v0.1.1", "0.1.-1"] {
            assert!(!newer(nonsense, "0.1.0"), "{nonsense}");
        }
        assert!(!newer("9.9.9", "not a version"), "he does not know what he is, so he stays");
    }

    fn feed(version: &str, url: &str, sha: &str) -> String {
        format!(r#"{{"version": "{version}", "url": "{url}", "sha256": "{sha}"}}"#)
    }

    #[test]
    fn a_release_is_read_only_when_it_is_whole() {
        let sha = "a".repeat(64);
        let good = feed("0.2.0", "https://updates.bohemia.systems/raccy/raccy-0.2.0.msi", &sha);
        let read = parse_feed(&good).expect("a whole release");
        assert_eq!(read.version, "0.2.0");
        assert_eq!(read.sha256, sha);
        assert_eq!(parse_feed(&feed("0.2.0", "https://updates.bohemia.systems/raccy/x.msi", "short")), None, "half a hash");
        assert_eq!(parse_feed(&feed("latest", "https://updates.bohemia.systems/raccy/x.msi", &sha)), None, "no version");
        assert_eq!(parse_feed("not json at all"), None);
        assert_eq!(parse_feed(r#"{"version": "0.2.0"}"#), None, "nothing to fetch");
    }

    // The package the updater would take, checked the way it checks it:
    // the hash the feed gives and the signature the build carries.
    // RACCY_TEST_MSI and RACCY_SIGNER come from the build scripts.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn the_built_package_is_what_the_feed_promises() {
        let path = std::path::PathBuf::from(std::env::var("RACCY_TEST_MSI").expect("the package to check"));
        let json = path.with_file_name("stable.json");
        let release = parse_feed(&std::fs::read_to_string(&json).expect("the feed beside it")).expect("a whole release");
        let mut file = std::fs::File::open(&path).expect("the package");
        let (hash, _) = platform::inspect::sha256(&mut file).expect("hashed");
        assert_eq!(hash, release.sha256, "the feed names this very package");
        let pin = std::env::var("RACCY_SIGNER").expect("the name, as build.ps1 sets it");
        assert!(platform::inspect::signed_by(&path, &pin), "signed by the one that builds him");
        assert!(!platform::inspect::signed_by(&path, &"0".repeat(64)), "and not by anybody else");
    }

    // A feed that could send him somewhere else could have him run anything.
    #[test]
    fn a_release_is_fetched_from_the_host_that_announced_it() {
        let sha = "b".repeat(64);
        for elsewhere in [
            "https://example.com/raccy/raccy-0.2.0.msi",
            "http://updates.bohemia.systems/raccy/raccy-0.2.0.msi",
            "https://updates.bohemia.systems.example.com/raccy.msi",
            "file:///tmp/raccy.msi",
        ] {
            assert_eq!(parse_feed(&feed("0.2.0", elsewhere, &sha)), None, "{elsewhere}");
        }
    }
}
