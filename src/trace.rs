use std::io::Write;
use std::sync::Mutex;

const RECORD_KEEP_BYTES: u64 = 1024 * 1024;

// One thread writes at a time, and the rotation is not torn by another.
static RECORD: Mutex<()> = Mutex::new(());

pub fn record_path() -> std::path::PathBuf {
    crate::platform::host::app_dir().join("raccy.log")
}

// The tests leave the record alone: it is the user's.
pub fn record(msg: impl FnOnce() -> String) {
    let text = msg();
    log(|| text.clone());
    if cfg!(test) {
        return;
    }
    let path = record_path();
    let _guard = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > RECORD_KEEP_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("ui");
        let _ = writeln!(f, "{} [{name}] {text}", crate::clock::record_stamp(crate::clock::unix_now()));
    }
}

#[cfg(feature = "trace")]
pub fn log(msg: impl FnOnce() -> String) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    let path = std::env::temp_dir().join("raccy-trace.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let thread = std::thread::current();
        let _ = writeln!(f, "{ms:>7} [{}] {}", thread.name().unwrap_or("ui"), msg());
    }
}

#[cfg(not(feature = "trace"))]
pub fn log(_msg: impl FnOnce() -> String) {}
