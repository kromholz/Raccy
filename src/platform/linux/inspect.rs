use std::ffi::{CString, c_char, c_void};
use std::io::Read;
use std::path::Path;

use super::command_output;
use crate::tools::inspect::{Origin, Signature, parse_zone};

// Nothing is kept in a cloud placeholder here.
pub fn in_cloud(_path: &Path) -> bool {
    false
}

// The SHA-256 of the whole file, and its first four bytes, which say
// whether it is a program.
pub fn sha256(file: &mut impl Read) -> Option<(String, Vec<u8>)> {
    crate::tools::inspect::sha256_of(file)
}

// Nothing signs a program on Linux: what answers the same question, who
// vouches for this file, is the package it came in, and a path in the Nix
// store carries the hash of everything that went into it.
pub fn signature(path: &Path, _size: u64) -> Signature {
    let Ok(path) = path.canonicalize() else { return Signature::NotSignable };
    // Everything under the Nix store is named after the hash of what built
    // it, and nothing may write there: the name is the vouching.
    if let Some(name) = nix_store_name(&path) {
        return Signature::Valid { signer: name, catalog: true };
    }
    let name = path.to_string_lossy().into_owned();
    for owner in OWNERS {
        let Some(package) = command_output(owner.program, &[owner.owns, &name]).and_then(|said| (owner.take)(&said)) else {
            continue;
        };
        // Owning the path is not the same as the file still being what the
        // package put there, which is what a signature says. The package
        // manager will compare it against its own record.
        return match verified(owner, &package, &name) {
            Some(true) => Signature::Valid { signer: package, catalog: true },
            Some(false) => Signature::Broken { signer: Some(package), why: crate::tools::inspect::Broken::Tampered },
            None => Signature::Unchecked,
        };
    }
    Signature::NotSignable
}

// Whether the package manager still recognises the file as its own, `None`
// when it cannot say. Each of them prints a line for a file that has changed
// and nothing for one that has not.
fn verified(owner: Owner, package: &str, path: &str) -> Option<bool> {
    let said = command_output(owner.program, &[owner.verify, package])?;
    Some(!said.lines().any(|line| line.contains(path)))
}

#[derive(Clone, Copy)]
struct Owner {
    program: &'static str,
    owns: &'static str,
    verify: &'static str,
    take: fn(&str) -> Option<String>,
}

const OWNERS: [Owner; 3] = [
    // coreutils: /usr/bin/ls
    Owner { program: "dpkg", owns: "-S", verify: "-V", take: |said| Some(said.split(':').next()?.trim().to_string()).filter(|name| !name.is_empty()) },
    // coreutils-9.5-1.fc41.x86_64
    Owner { program: "rpm", owns: "-qf", verify: "-V", take: |said| Some(said.lines().next()?.trim().to_string()).filter(|name| !name.is_empty() && !name.contains(' ')) },
    // /usr/bin/ls is owned by coreutils 9.5-1
    // The version follows the name, and only the name may be asked about.
    Owner { program: "pacman", owns: "-Qo", verify: "-Qkk", take: |said| Some(said.split(" is owned by ").nth(1)?.split_whitespace().next()?.to_string()).filter(|name| !name.is_empty()) },
];

// `/nix/store/<hash>-coreutils-9.5/bin/ls` was built from a recipe whose
// every input is in that hash: the name without the hash says what it is.
fn nix_store_name(path: &Path) -> Option<String> {
    let rest = path.strip_prefix("/nix/store").ok()?;
    let entry = rest.components().next()?.as_os_str().to_string_lossy();
    let (hash, name) = entry.split_once('-')?;
    (hash.len() == 32 && !name.is_empty()).then(|| name.to_string())
}

// No signed copy of his own is kept here: only a test asks.
#[cfg_attr(not(test), allow(dead_code))]
pub fn signed_by(_path: &Path, _signer: &str) -> bool {
    false
}

unsafe extern "C" {
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize) -> isize;
}

fn xattr(path: &Path, name: &str) -> Option<String> {
    let path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let name = CString::new(name).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = unsafe { getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).trim().to_string())
}

// The mark browsers leave on downloads, fed to the same parser as the Windows
// zone note.
pub fn origin(path: &Path) -> Option<Origin> {
    let host = xattr(path, "user.xdg.origin.url")?;
    let referrer = xattr(path, "user.xdg.referrer.url").unwrap_or_default();
    let zone = if host.starts_with("http://") || host.starts_with("https://") { 3 } else { 0 };
    parse_zone(&format!("[ZoneTransfer]\nZoneId={zone}\nHostUrl={host}\nReferrerUrl={referrer}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nix_store_path_says_what_it_holds() {
        let store = Path::new("/nix/store/5h437da0rwkpj5zii6h3bqq82x6qqk1b-hyprland-0.55.4/bin/Hyprland");
        assert_eq!(nix_store_name(store).as_deref(), Some("hyprland-0.55.4"));
        assert_eq!(nix_store_name(Path::new("/usr/bin/ls")), None);
        assert_eq!(nix_store_name(Path::new("/nix/store/short-name/bin/x")), None, "that is no store hash");
    }

    #[test]
    fn each_package_manager_names_the_owner() {
        assert_eq!((OWNERS[0].take)("coreutils: /usr/bin/ls"), Some("coreutils".into()));
        assert_eq!((OWNERS[1].take)("coreutils-9.5-1.fc41.x86_64\n"), Some("coreutils-9.5-1.fc41.x86_64".into()));
        assert_eq!((OWNERS[1].take)("file /usr/bin/x is not owned by any package"), None);
        // Only the name, which is what asking after it again takes.
        assert_eq!((OWNERS[2].take)("/usr/bin/ls is owned by coreutils 9.5-1"), Some("coreutils".into()));
        assert_eq!((OWNERS[2].take)("error: No package owns /usr/bin/x"), None);
    }
}
