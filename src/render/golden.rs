use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

pub fn check(name: &str, bmp: &[u8]) {
    let golden = root().join("golden").join(format!("{name}.bmp"));
    if std::env::var_os("RACCY_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, bmp).unwrap();
        println!("wrote {}", golden.display());
        return;
    }
    let kept = std::fs::read(&golden).unwrap_or_else(|_| panic!("no golden sheet at {}: RACCY_UPDATE_GOLDEN=1 cargo test golden makes it", golden.display()));
    if kept != bmp {
        let out = root().join("target").join("golden").join(format!("{name}.bmp"));
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(&out, bmp).unwrap();
        panic!(
            "{name} draws differently from golden\\{name}.bmp; the new sheet is at {}. Meant? RACCY_UPDATE_GOLDEN=1 cargo test golden",
            out.display()
        );
    }
}

// A 24-bit BMP of an RGB image, rows top to bottom.
pub fn bmp(width: usize, height: usize, rgb: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + row * height);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + row * height) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&[1, 0, 24, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&((row * height) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..height).rev() {
        let start = out.len();
        for x in 0..width {
            let [r, g, b] = rgb(x, y);
            out.extend_from_slice(&[b, g, r]);
        }
        out.resize(start + row, 0);
    }
    out
}
