use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CERT_E_EXPIRED, CERT_E_UNTRUSTEDROOT, HANDLE, TRUST_E_BAD_DIGEST, TRUST_E_EXPLICIT_DISTRUST, TRUST_E_NOSIGNATURE,
};
use windows_sys::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash,
    CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptCreateHash,
    BCryptDestroyHash, BCryptFinishHash, BCryptHashData, BCryptOpenAlgorithmProvider, CERT_CONTEXT,
    CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_NAME_STR_REVERSE_FLAG, CERT_X500_NAME_STR, CertGetNameStringW, CertNameToStrW,
    X509_ASN_ENCODING,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows_sys::core::PCWSTR;

use crate::tools::inspect::{Broken, Origin, Signature, parse_zone};

const TRUST_E_SUBJECT_FORM_UNKNOWN: i32 = 0x800B_0003_u32 as i32;
const TRUST_E_PROVIDER_UNKNOWN: i32 = 0x800B_0001_u32 as i32;
const CERT_E_CHAINING: i32 = 0x800B_010A_u32 as i32;
const CERT_E_REVOKED: i32 = 0x800B_010C_u32 as i32;
const TRUST_E_CERT_SIGNATURE: i32 = 0x8009_6004_u32 as i32;
const TRUST_E_TIME_STAMP: i32 = 0x8009_6005_u32 as i32;
// Files only in the cloud: reading one would download it first.
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
// Bigger files are not hashed again to look for a Windows catalog: those
// sign Windows' own, small files.
const CATALOG_MAX: u64 = 64 * 1024 * 1024;

pub fn in_cloud(path: &Path) -> bool {
    let attributes = std::fs::metadata(path).map_or(0, |m| std::os::windows::fs::MetadataExt::file_attributes(&m));
    attributes & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// The SHA-256 of the whole file, and its first four bytes, which say
// whether it is a program.
pub fn sha256(file: &mut impl Read) -> Option<(String, Vec<u8>)> {
    unsafe {
        let mut algorithm: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0) != 0 {
            return None;
        }
        let mut hash: BCRYPT_HASH_HANDLE = std::ptr::null_mut();
        let mut out = None;
        if BCryptCreateHash(algorithm, &mut hash, std::ptr::null_mut(), 0, std::ptr::null(), 0, 0) == 0 {
            let mut buf = vec![0u8; 1 << 20];
            let (mut head, mut ok) = (Vec::new(), true);
            loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if head.len() < 4 {
                            head.extend_from_slice(&buf[..n.min(4 - head.len())]);
                        }
                        ok &= BCryptHashData(hash, buf.as_ptr(), n as u32, 0) == 0;
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            let mut digest = [0u8; 32];
            if ok && BCryptFinishHash(hash, digest.as_mut_ptr(), digest.len() as u32, 0) == 0 {
                out = Some((hex(&digest), head));
            }
            BCryptDestroyHash(hash);
        }
        BCryptCloseAlgorithmProvider(algorithm, 0);
        out
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

// The signature in the file, or failing that in a Windows catalog, which is
// how most of Windows' own files are signed.
pub fn signature(path: &Path, size: u64) -> Signature {
    let wide = wide_path(path);
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let (rc, signer) = verify(WTD_CHOICE_FILE, WINTRUST_DATA_0 { pFile: &mut info }, name_of);
    match rc {
        0 => Signature::Valid { signer: signer.unwrap_or_default(), catalog: false },
        TRUST_E_NOSIGNATURE if size <= CATALOG_MAX => catalog(path).unwrap_or(Signature::Unsigned),
        TRUST_E_NOSIGNATURE => Signature::Unsigned,
        TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN if size <= CATALOG_MAX => catalog(path).unwrap_or(Signature::NotSignable),
        TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => Signature::NotSignable,
        rc => broken(rc).map_or(Signature::Unchecked, |why| Signature::Broken { signer, why }),
    }
}

// What a failed check says about the signature; `None` when the check
// itself went wrong and says nothing about it.
fn broken(rc: i32) -> Option<Broken> {
    Some(match rc {
        TRUST_E_BAD_DIGEST => Broken::Tampered,
        TRUST_E_EXPLICIT_DISTRUST | CERT_E_REVOKED => Broken::Distrusted,
        CERT_E_EXPIRED => Broken::Expired,
        CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING => Broken::UntrustedRoot,
        TRUST_E_CERT_SIGNATURE | TRUST_E_TIME_STAMP => Broken::Other,
        _ => return None,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn signed_by(path: &Path, signer: &str) -> bool {
    let wide = wide_path(path);
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let (rc, subject) = verify(WTD_CHOICE_FILE, WINTRUST_DATA_0 { pFile: &mut info }, subject_of);
    rc == 0 && subject.is_some_and(|subject| subject == signer)
}

// Reversed, so it reads from the common name outwards: the way the file's
// properties show it and the way the pin is written.
const X500_AS_SHOWN: u32 = CERT_X500_NAME_STR | CERT_NAME_STR_REVERSE_FLAG;

fn subject_of(cert: *const CERT_CONTEXT) -> Option<String> {
    unsafe {
        let name = &(*(*cert).pCertInfo).Subject;
        let wanted = CertNameToStrW(X509_ASN_ENCODING, name, X500_AS_SHOWN, std::ptr::null_mut(), 0);
        if wanted <= 1 {
            return None;
        }
        let mut text = vec![0u16; wanted as usize];
        let n = CertNameToStrW(X509_ASN_ENCODING, name, X500_AS_SHOWN, text.as_mut_ptr(), wanted);
        (n > 1).then(|| String::from_utf16_lossy(&text[..n as usize - 1]))
    }
}

// Revocation is not checked.
fn verify<T>(choice: u32, subject: WINTRUST_DATA_0, read: impl FnOnce(*const CERT_CONTEXT) -> Option<T>) -> (i32, Option<T>) {
    unsafe {
        let mut data: WINTRUST_DATA = std::mem::zeroed();
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = choice;
        data.Anonymous = subject;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let rc = WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        let signer = signer_of(data.hWVTStateData).and_then(read);
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        (rc, signer)
    }
}

// The signing certificate, from a verification's state; it lives as long as
// the state.
unsafe fn signer_of(state: HANDLE) -> Option<*const CERT_CONTEXT> {
    unsafe {
        if state.is_null() {
            return None;
        }
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        (!cert.is_null()).then_some(cert)
    }
}

fn name_of(cert: *const CERT_CONTEXT) -> Option<String> {
    unsafe {
        let mut name = [0u16; 256];
        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), name.as_mut_ptr(), name.len() as u32);
        (n > 1).then(|| String::from_utf16_lossy(&name[..n as usize - 1]))
    }
}

// A catalog signature, looked up by the file's hash: SHA-256 catalogs first,
// then the older SHA-1 ones.
fn catalog(path: &Path) -> Option<Signature> {
    [BCRYPT_SHA256_ALGORITHM, std::ptr::null()].into_iter().find_map(|algorithm| catalog_with(path, algorithm))
}

fn catalog_with(path: &Path, algorithm: PCWSTR) -> Option<Signature> {
    let file = File::open(path).ok()?;
    let handle = file.as_raw_handle() as HANDLE;
    let wide = wide_path(path);
    unsafe {
        let mut admin: isize = 0;
        if CryptCATAdminAcquireContext2(&mut admin, std::ptr::null(), algorithm, std::ptr::null(), 0) == 0 {
            return None;
        }
        let mut len = 0u32;
        CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, std::ptr::null_mut(), 0);
        let mut hash = vec![0u8; len as usize];
        let mut result = None;
        if len > 0 && CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, hash.as_mut_ptr(), 0) != 0 {
            let context = CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), len, 0, std::ptr::null_mut());
            if context != 0 {
                let mut info: CATALOG_INFO = std::mem::zeroed();
                info.cbStruct = size_of::<CATALOG_INFO>() as u32;
                if CryptCATCatalogInfoFromContext(context, &mut info, 0) != 0 {
                    let tag: Vec<u16> = hex(&hash).to_uppercase().encode_utf16().chain(Some(0)).collect();
                    let mut member = WINTRUST_CATALOG_INFO {
                        cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                        dwCatalogVersion: 0,
                        pcwszCatalogFilePath: info.wszCatalogFile.as_ptr(),
                        pcwszMemberTag: tag.as_ptr(),
                        pcwszMemberFilePath: wide.as_ptr(),
                        hMemberFile: handle,
                        pbCalculatedFileHash: hash.as_mut_ptr(),
                        cbCalculatedFileHash: len,
                        pcCatalogContext: std::ptr::null_mut(),
                        hCatAdmin: admin,
                    };
                    let (rc, signer) = verify(WTD_CHOICE_CATALOG, WINTRUST_DATA_0 { pCatalog: &mut member }, name_of);
                    result = Some(match rc {
                        0 => Signature::Valid { signer: signer.unwrap_or_default(), catalog: true },
                        rc => broken(rc).map_or(Signature::Unchecked, |why| Signature::Broken { signer, why }),
                    });
                }
                CryptCATAdminReleaseCatalogContext(admin, context, 0);
            }
        }
        CryptCATAdminReleaseContext(admin, 0);
        result
    }
}

pub fn origin(path: &Path) -> Option<Origin> {
    let mut stream = OsString::from(path.as_os_str());
    stream.push(":Zone.Identifier");
    parse_zone(&std::fs::read_to_string(stream).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dribble<'a>(&'a [u8]);

    impl Read for Dribble<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(2).min(out.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_file_with_nothing_in_it_still_has_a_hash() {
        let (hash, head) = sha256(&mut b"".as_slice()).expect("nothing hashes too");
        assert_eq!(hash, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert!(head.is_empty());
    }

    #[test]
    fn the_head_of_a_file_is_its_first_four_bytes_however_it_is_read() {
        let program = b"MZ\x90\x00 and the rest of it";
        let (whole, head) = sha256(&mut program.as_slice()).expect("hashed");
        assert_eq!(head, b"MZ\x90\x00");
        let (piecemeal, head) = sha256(&mut Dribble(program)).expect("hashed");
        assert_eq!(head, b"MZ\x90\x00", "the head is collected across reads");
        assert_eq!(whole, piecemeal, "and the hash is the same either way");
    }

    #[test]
    fn a_signature_that_does_not_hold_says_why() {
        assert_eq!(broken(TRUST_E_BAD_DIGEST), Some(Broken::Tampered));
        assert_eq!(broken(TRUST_E_EXPLICIT_DISTRUST), Some(Broken::Distrusted));
        assert_eq!(broken(CERT_E_REVOKED), Some(Broken::Distrusted));
        assert_eq!(broken(CERT_E_EXPIRED), Some(Broken::Expired));
        assert_eq!(broken(CERT_E_UNTRUSTEDROOT), Some(Broken::UntrustedRoot));
        assert_eq!(broken(CERT_E_CHAINING), Some(Broken::UntrustedRoot));
        assert_eq!(broken(TRUST_E_TIME_STAMP), Some(Broken::Other));
        // Not a verdict on the signature: a file with none, or a check that
        // could not be made at all.
        assert_eq!(broken(0), None);
        assert_eq!(broken(TRUST_E_NOSIGNATURE), None);
    }

    // Prints what belongs in RACCY_SIGNER, read off a signed build.
    #[test]
    #[ignore]
    fn what_name_signed_this() {
        let path = std::env::var_os("RACCY_LOOK_AT").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(r"target\release\raccy.exe")
        });
        let wide = wide_path(&path);
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide.as_ptr(),
            hFile: std::ptr::null_mut(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let (rc, subject) = verify(WTD_CHOICE_FILE, WINTRUST_DATA_0 { pFile: &mut info }, subject_of);
        println!("{}\n  trust: {rc:#x}\n  subject: {subject:?}", path.display());
    }

    // The name carries diacritics, and Windows write it the other way round
    // from everything else that shows a subject.
    #[test]
    #[ignore]
    fn the_signing_name_is_read_the_way_it_is_written() {
        let path = std::env::var_os("RACCY_LOOK_AT").map(std::path::PathBuf::from).expect("RACCY_LOOK_AT");
        let want = std::env::var("RACCY_SIGNER").expect("RACCY_SIGNER");
        assert!(signed_by(&path, &want), "signed by {want}");
        assert!(!signed_by(&path, &want.to_lowercase()), "and not by the same name in another case");
        let mut reversed: Vec<&str> = want.split(", ").collect();
        reversed.reverse();
        assert!(!signed_by(&path, &reversed.join(", ")), "nor by the same parts the other way round");
    }
}
