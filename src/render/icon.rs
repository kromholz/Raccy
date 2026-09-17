use crate::pet::Activity;
use crate::render::sprite::{self, Gait, Idle, NEON_CYAN, Pose, Rgb, Visor};

type Image = Vec<Option<Rgb>>;

// Head and ushanka: rows 0..24, columns 4..28 of the sprite.
const HEAD: (usize, usize) = (4, 0);
const HEAD_SIZE: usize = 24;

fn portrait() -> Image {
    // Frame 175 of a feed puts the visor scanner in the middle and catches the
    // antenna flashing with a bite.
    let pose = Pose { activity: Activity::Content, frame: 175, busy: true, feast: true, squash: false, morsel: None, led: NEON_CYAN, alert: false, petted: false, stage: 1, neglect: 0.0, idle: Idle::Still, gait: Gait::Still, facing_left: false, visor: Visor::Down };
    sprite::draw(&pose).iter().flatten().map(|p| p.map(|px| px.c)).collect()
}

fn crop(full: &Image, (x0, y0): (usize, usize), size: usize) -> Image {
    (0..size).flat_map(|y| (0..size).map(move |x| full[(y0 + y) * sprite::SIZE + x0 + x])).collect()
}

// Nearest-neighbour, so pixels stay pixels.
fn resize(src: &Image, from: usize, to: usize) -> Image {
    (0..to).flat_map(|y| (0..to).map(move |x| src[(y * from / to) * from + x * from / to])).collect()
}

fn images() -> Vec<(usize, Image)> {
    let full = portrait();
    let head = crop(&full, HEAD, HEAD_SIZE);
    vec![
        (16, resize(&head, HEAD_SIZE, 16)),
        (24, head.clone()),
        (32, full.clone()),
        (48, resize(&head, HEAD_SIZE, 48)),
        (64, resize(&full, sprite::SIZE, 64)),
        (256, resize(&full, sprite::SIZE, 256)),
    ]
}

// One 32-bit DIB entry: header with doubled height, bottom-up BGRA, then
// the AND mask (set bits are transparent).
fn dib(size: usize, image: &Image) -> Vec<u8> {
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + size * size * 4 + mask_row * size);
    let header: [u32; 10] = [40, size as u32, 2 * size as u32, 0, 0, (size * size * 4 + mask_row * size) as u32, 0, 0, 0, 0];
    for (i, field) in header.iter().enumerate() {
        match i {
            // biPlanes and biBitCount share the fourth slot as two u16s.
            3 => {
                out.extend_from_slice(&1u16.to_le_bytes());
                out.extend_from_slice(&32u16.to_le_bytes());
            }
            4 => out.extend_from_slice(&0u32.to_le_bytes()),
            _ => out.extend_from_slice(&field.to_le_bytes()),
        }
    }
    out.truncate(40);
    for y in (0..size).rev() {
        for x in 0..size {
            match image[y * size + x] {
                Some([r, g, b]) => out.extend_from_slice(&[b, g, r, 255]),
                None => out.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..size {
            if image[y * size + x].is_none() {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

pub fn encode() -> Vec<u8> {
    let entries: Vec<(usize, Vec<u8>)> = images().iter().map(|(size, img)| (*size, dib(*size, img))).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for (size, data) in &entries {
        let edge = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[edge, edge, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &entries {
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/raccy.ico");
    const LOGO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/raccy.png");
    const BANNER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/banner.bmp");
    const DIALOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/dialog.bmp");

    // The wizard writes its own black text over both bitmaps, so everything
    // under that text stays pale.
    const PAPER: Rgb = [0xf4, 0xf2, 0xf8];
    const BAND: u32 = 164;

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn entries_are_well_formed() {
        let ico = encode();
        assert_eq!((u16_at(&ico, 0), u16_at(&ico, 2), u16_at(&ico, 4)), (0, 1, 6));
        for i in 0..6 {
            let e = 6 + 16 * i;
            let size = if ico[e] == 0 { 256 } else { ico[e] as usize };
            let (len, offset) = (u32_at(&ico, e + 8) as usize, u32_at(&ico, e + 12) as usize);
            assert!(offset + len <= ico.len(), "entry {i} in bounds");
            assert_eq!(u32_at(&ico, offset), 40, "BITMAPINFOHEADER");
            assert_eq!(u32_at(&ico, offset + 4) as usize, size);
            assert_eq!(u32_at(&ico, offset + 8) as usize, 2 * size, "height covers the mask");
            assert_eq!(u16_at(&ico, offset + 14), 32);
        }
    }

    #[test]
    fn committed_icon_matches_the_sprite() {
        let committed = std::fs::read(PATH).expect("assets/raccy.ico exists");
        assert!(committed == encode(), "the sprite changed; run `cargo test icon -- --ignored` to redraw the icon");
    }

    #[test]
    #[ignore]
    fn write_icon() {
        std::fs::create_dir_all(std::path::Path::new(PATH).parent().unwrap()).unwrap();
        std::fs::write(PATH, encode()).unwrap();
    }

    fn png_of(src: &Image, from: usize, edge: usize) -> image::RgbaImage {
        let pixels = resize(src, from, edge);
        let mut png = image::RgbaImage::new(edge as u32, edge as u32);
        for (at, cell) in pixels.iter().enumerate() {
            let [r, g, b] = cell.unwrap_or([0, 0, 0]);
            let alpha = if cell.is_some() { 255 } else { 0 };
            png.put_pixel((at % edge) as u32, (at / edge) as u32, image::Rgba([r, g, b, alpha]));
        }
        png
    }

    fn logo() -> image::RgbaImage {
        png_of(&portrait(), sprite::SIZE, sprite::SIZE * 4)
    }

    #[test]
    #[ignore]
    fn writes_the_iconset() {
        let full = portrait();
        let head = crop(&full, HEAD, HEAD_SIZE);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/icon/Raccy.iconset");
        std::fs::create_dir_all(&dir).expect("somewhere to write");
        for edge in [16usize, 32, 64, 128, 256, 512, 1024] {
            let (from, src) = if edge <= 32 { (HEAD_SIZE, &head) } else { (sprite::SIZE, &full) };
            let png = png_of(src, from, edge);
            for name in names_for(edge) {
                png.save(dir.join(name)).expect("write the icon");
            }
        }
        println!("{}", dir.display());
    }

    #[test]
    #[ignore]
    fn write_logo() {
        logo().save(LOGO).expect("write the logo");
    }

    fn paint(canvas: &mut image::RgbImage, src: &Image, from: usize, edge: usize, at: (u32, u32)) {
        for (i, cell) in resize(src, from, edge).iter().enumerate() {
            if let Some([r, g, b]) = *cell {
                canvas.put_pixel(at.0 + (i % edge) as u32, at.1 + (i / edge) as u32, image::Rgb([r, g, b]));
            }
        }
    }

    fn banner() -> image::RgbImage {
        let mut bmp = image::RgbImage::from_pixel(493, 58, image::Rgb(PAPER));
        paint(&mut bmp, &crop(&portrait(), HEAD, HEAD_SIZE), HEAD_SIZE, 48, (433, 5));
        bmp
    }

    fn dialog() -> image::RgbImage {
        let mut bmp = image::RgbImage::from_pixel(493, 312, image::Rgb(PAPER));
        for y in 0..312 {
            for x in 0..BAND {
                bmp.put_pixel(x, y, image::Rgb(sprite::OUTLINE));
            }
        }
        paint(&mut bmp, &portrait(), sprite::SIZE, 128, (18, 92));
        bmp
    }

    #[test]
    #[ignore]
    fn write_wizard_bitmaps() {
        banner().save(BANNER).expect("write the banner");
        dialog().save(DIALOG).expect("write the dialog");
    }

    #[test]
    fn committed_wizard_bitmaps_match_the_sprite() {
        for (path, drawn) in [(BANNER, banner()), (DIALOG, dialog())] {
            let committed = image::open(path).unwrap_or_else(|_| panic!("{path} exists")).to_rgb8();
            assert!(committed == drawn, "the sprite changed; run `cargo test icon -- --ignored` to redraw {path}");
        }
    }

    #[test]
    fn committed_logo_matches_the_sprite() {
        let committed = image::open(LOGO).expect("assets/raccy.png exists").to_rgba8();
        assert!(committed == logo(), "the sprite changed; run `cargo test icon -- --ignored` to redraw the logo");
    }

    // iconutil refuses any name but these ten.
    fn names_for(edge: usize) -> Vec<String> {
        const PLAIN: [usize; 5] = [16, 32, 128, 256, 512];
        let mut names = Vec::new();
        if PLAIN.contains(&edge) {
            names.push(format!("icon_{edge}x{edge}.png"));
        }
        if PLAIN.contains(&(edge / 2)) {
            let half = edge / 2;
            names.push(format!("icon_{half}x{half}@2x.png"));
        }
        names
    }
}
