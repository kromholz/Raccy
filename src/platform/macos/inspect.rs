use std::io::Read;
use std::path::Path;

use super::{command_output, command_said};
use crate::tools::inspect::{Origin, Signature, parse_zone};

// Nothing is kept in a cloud placeholder here that reading would fetch.
pub fn in_cloud(_path: &Path) -> bool {
    false
}

// The SHA-256 of the whole file, and its first four bytes, which say
// whether it is a program.
pub fn sha256(file: &mut impl Read) -> Option<(String, Vec<u8>)> {
    crate::tools::inspect::sha256_of(file)
}

// Who vouches for a program here: the signature Apple's own tools read, and
// the authority that put it there.
pub fn signature(path: &Path, _size: u64) -> Signature {
    let Ok(path) = path.canonicalize() else { return Signature::NotSignable };
    let name = path.display().to_string();
    // Whatever codesign has to say about a signature it says on the error
    // stream, success and failure alike, and nothing at all on the other one.
    let Some(said) = command_said("codesign", &["-dv", "--verbose=2", &name]) else {
        return Signature::NotSignable;
    };
    let Some(signer) = authority_of(&said) else { return Signature::NotSignable };
    match command_output("codesign", &["--verify", "--strict", &name]) {
        Some(_) => Signature::Valid { signer, catalog: false },
        None => Signature::Broken { signer: Some(signer), why: crate::tools::inspect::Broken::Tampered },
    }
}

// `Authority=Developer ID Application: Someone (TEAMID)` among the lines
// codesign prints about itself, or `Authority=Software Signing` for Apple's
// own. A program signed by nothing but the machine it was built on has none.
fn authority_of(said: &str) -> Option<String> {
    said.lines().find_map(|l| l.trim().strip_prefix("Authority=")).map(str::to_string).filter(|a| !a.is_empty())
}

// Whether the file carries the mark of whoever builds Raccy. Here that mark
// is the authority codesign names; on Windows it is the hash of a certificate.
#[cfg_attr(not(test), allow(dead_code))]
pub fn signed_by(path: &Path, signer: &str) -> bool {
    let name = path.display().to_string();
    command_output("codesign", &["--verify", "--strict", &name]).is_some()
        && command_said("codesign", &["-dv", "--verbose=2", &name]).and_then(|said| authority_of(&said)).is_some_and(|a| a.contains(signer))
}

// Where a file came from. The mark a browser leaves says only that it came
// from the web at all; the address is kept apart from it, in what the browser
// told Spotlight about the file.
pub fn origin(path: &Path) -> Option<Origin> {
    let name = path.display().to_string();
    command_output("xattr", &["-p", "com.apple.quarantine", &name])?;
    let (host, referrer) = command_output("mdls", &["-raw", "-name", "kMDItemWhereFroms", &name]).and_then(|said| where_from(&said)).unwrap_or_default();
    parse_zone(&format!("[ZoneTransfer]\nZoneId={}\nHostUrl={host}\nReferrerUrl={referrer}\n", crate::tools::inspect::ZONE_INTERNET))
}

// The addresses as mdls prints them, one to a line and quoted: what the file
// was fetched from first, and the page it was linked from after it.
fn where_from(said: &str) -> Option<(String, String)> {
    let mut urls = said.lines().filter_map(|l| Some(l.trim().trim_end_matches(',').strip_prefix('"')?.strip_suffix('"')?.to_string()));
    Some((urls.next()?, urls.next().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codesign_names_the_authority_that_vouches() {
        let said = "Executable=/bin/ls\nIdentifier=com.apple.ls\nCodeDirectory v=20400 size=741\nAuthority=Software Signing\nAuthority=Apple Code Signing Certification Authority\nTeamIdentifier=not set\n";
        assert_eq!(authority_of(said).as_deref(), Some("Software Signing"), "the first authority is the one that signed it");
        let adhoc = "Executable=/tmp/a.out\nIdentifier=a\nSignature=adhoc\n";
        assert_eq!(authority_of(adhoc), None, "signed by nothing but the machine it was built on");
    }

    #[test]
    fn spotlight_says_where_a_download_came_from() {
        let said = "(\n    \"https://example.com/thing.dmg\",\n    \"https://example.com/page\"\n)\n";
        assert_eq!(where_from(said), Some(("https://example.com/thing.dmg".into(), "https://example.com/page".into())));
        assert_eq!(where_from("(null)\n"), None, "nothing was ever told about it");
    }

    #[test]
    #[ignore]
    fn what_this_machine_says_about_its_own_files() {
        for file in ["/bin/ls", "/usr/bin/codesign", "/usr/local/bin/nothing"] {
            println!("{file}: {:?}", signature(Path::new(file), 0));
        }
        // A file to ask after, named in RACCY_LOOK_AT; nothing is gone
        // through on its own.
        if let Some(look) = std::env::var_os("RACCY_LOOK_AT") {
            let path = std::path::PathBuf::from(look);
            println!("{}: {:?} {:?}", path.display(), signature(&path, 0), origin(&path));
        }
    }
}
