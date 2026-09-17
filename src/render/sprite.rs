use crate::net::Kind;
use crate::pet::Activity;

pub const SIZE: usize = 32;
pub const BITE_FRAMES: u64 = 25;
const LED_WINK_FRAMES: u64 = 30;

#[rustfmt::skip]
const BASE: [&str; SIZE] = [
    "................................",
    ".............LL.................",
    ".............LL.................",
    "........K....A.........K........",
    ".......KMK...A........KMK.......",
    "......KGMGK..A.......KGMGK......",
    "......KGMGKKKKKKKKKKKKGMGK......",
    ".....KGHHHHHHHHBBHHHHHHHHGK.....",
    ".....KHHHHHHHHHBBHHHHHHHHHK.....",
    "....KhhhhhhhhhhhhhhhhhhhhhhK....",
    "....KhhhhhhhhhhhhhhhhhhhhhhK....",
    "....KhGGKKKKKKKKKKKKKKKKGGhK....",
    "....KhMMKVVVVVVVVVVVVVVKMMhK....",
    "....KhMMKVVVVVVVVVVVVVVKMMhK....",
    "....KhMMKKKKKKKKKKKKKKKKMMhK....",
    "....KhWWWMMMMWWWWWWMMMMWWWhK....",
    ".....KWWWWWWWWWNNWWWWWWWWWK.....",
    "......KWWWWWWKWWWWKWWWWWWK......",
    ".......KWWWWWWKKKKWWWWWWK...KKK.",
    "........KKKKKKKKKKKKKKKK...KTTTK",
    ".......KJSJJJJJJJJJJJJJSJK.KGGGK",
    "......KJSJJJJggggggJJJJSJJKKTTTK",
    ".....KJJSJJJJggggggJJJJSJJJKGGGK",
    ".....KJJSJJJJggggggJJJJSJJJKTTTK",
    ".....KJJSJJJJggggggJJJJSJJJKGGGK",
    ".....KJJSJJJJggggggJJJJSJJJKTTTK",
    ".....KJJSJJJJggggggJJJJSJJJKGGK.",
    "......KJJJJJJJJJJJJJJJJJJJKTTK..",
    "......KJPPPPJJJJJJJJPPPPJJKKK...",
    ".......KPPPPKKKKKKKKPPPPK.......",
    "........KKKK........KKKK........",
    "................................",
];

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px {
    pub c: Rgb,
    pub glow: bool,
}

pub const OUTLINE: Rgb = [0x0b, 0x08, 0x12];
const FUR: Rgb = [0x4a, 0x44, 0x58];
const CHEST: Rgb = [0x8a, 0x82, 0xa0];
const HAT: Rgb = [0x2b, 0x24, 0x36];
const TRIM: Rgb = [0x6e, 0x64, 0x85];
const MASK: Rgb = [0x14, 0x0f, 0x1d];
const MUZZLE: Rgb = [0xcf, 0xc7, 0xde];
const NOSE: Rgb = [0x05, 0x03, 0x08];
const METAL: Rgb = [0x5a, 0x56, 0x70];
const JACKET: Rgb = [0x1c, 0x17, 0x26];
const TONGUE: Rgb = [0xff, 0x4f, 0x7b];
const LED_OFF: Rgb = [0x2a, 0x24, 0x36];
pub const NEON_RED: Rgb = [0xff, 0x24, 0x48];
pub const NEON_CYAN: Rgb = [0x22, 0xf3, 0xff];
const VISOR_DIM: Rgb = [0xa0, 0x14, 0x2f];
const VISOR_SCAN: Rgb = [0xff, 0x8a, 0xa0];
const VISOR_OFF: Rgb = [0x2a, 0x0d, 0x16];
const VISOR_ALERT: Rgb = [0xff, 0xb0, 0x20];
const COAT: Rgb = [0x3a, 0x2a, 0x26];
const DIRT: Rgb = [0x5a, 0x4a, 0x3a];
const CHROME: Rgb = [0xa8, 0xb0, 0xc0];
const EYE: Rgb = [0xe8, 0xe4, 0xf2];
const PCB: Rgb = [0x1f, 0x6b, 0x3a];
const SPARK: Rgb = [0xff, 0xe0, 0x6a];
const PAPER: Rgb = [0xd8, 0xd0, 0xbc];
const PAPER_FOLD: Rgb = [0xb4, 0xac, 0x98];
const INK: Rgb = [0x3a, 0x34, 0x44];
const SCREEN: Rgb = [0x10, 0x30, 0x18];
const PIXEL_GREEN: Rgb = [0x7c, 0xff, 0x6a];
const BUN: Rgb = [0xd9, 0x9a, 0x4e];
const CRUST: Rgb = [0x9a, 0x5e, 0x2a];

pub fn flavour(kind: Kind) -> Rgb {
    match kind {
        Kind::Web => [0x4d, 0xa6, 0xff],
        Kind::PlainWeb => [0xff, 0x9f, 0x43],
        Kind::Ssh => [0x5c, 0xff, 0x8a],
        Kind::RemoteDesktop | Kind::WinRm => [0xb0, 0x7c, 0xff],
        Kind::Dns => [0xff, 0xe0, 0x4d],
        Kind::Mail => [0xff, 0x7e, 0xd6],
        Kind::FileShare => [0xe0, 0xa8, 0x70],
        Kind::Database => [0x3d, 0xff, 0xd0],
        Kind::Git => [0xff, 0x6a, 0x3c],
        Kind::Chat => [0xb8, 0xff, 0x5a],
        Kind::Call => [0xff, 0x4d, 0xf0],
        Kind::Game => [0xff, 0x40, 0x40],
        Kind::Other(_) => [0xee, 0xee, 0xff],
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Idle {
    #[default]
    Still,
    LookLeft,
    LookRight,
    Yawn,
    Stretch,
    Tinker,
    Read,
    Game,
    Snack,
}

impl Idle {
    pub fn is_hobby(self) -> bool {
        matches!(self, Idle::Tinker | Idle::Read | Idle::Game | Idle::Snack)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gait {
    #[default]
    Still,
    Walking,
    Jumping,
    Sitting,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visor {
    #[default]
    Down,
    Half,
    Up,
}

type Grid = [[Option<Px>; SIZE]; SIZE];

#[derive(Clone, Copy)]
pub struct Pose {
    pub activity: Activity,
    // Ten frames a second.
    pub frame: u64,
    pub busy: bool,
    pub feast: bool,
    pub squash: bool,
    pub morsel: Option<Rgb>,
    pub led: Rgb,
    pub alert: bool,
    pub petted: bool,
    // 0 pup, 1 fence, 2 hacker, 3 legend.
    pub stage: u8,
    // 0 when well kept, up to 1 after long hunger.
    pub neglect: f32,
    pub idle: Idle,
    pub gait: Gait,
    pub facing_left: bool,
    pub visor: Visor,
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

pub fn draw(pose: &Pose) -> [[Option<Px>; SIZE]; SIZE] {
    let rows: Vec<&[u8]> = BASE.iter().map(|r| r.as_bytes()).collect();
    let mut px = [[None; SIZE]; SIZE];
    let asleep = pose.activity == Activity::Sleeping && pose.gait == Gait::Still;
    let wag = (pose.frame / 5) % 2 == 1 && !asleep;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut c = rows[y][x];
            if wag && x >= 28 {
                c = match c {
                    b'G' => b'T',
                    b'T' => b'G',
                    other => other,
                };
            }
            let (color, glow) = match c {
                b'K' => (OUTLINE, false),
                // A hacker's tail rings alternate red and cyan.
                b'G' if pose.stage >= 2 && x >= 28 => (NEON_CYAN, true),
                b'G' => (FUR, false),
                b'g' => (CHEST, false),
                b'H' => (HAT, false),
                b'h' => (TRIM, false),
                b'M' | b'P' => (MASK, false),
                b'W' => (MUZZLE, false),
                b'N' => (NOSE, false),
                b'A' => (METAL, false),
                b'J' if pose.stage >= 3 => (COAT, false),
                b'J' => (JACKET, false),
                b'S' => (NEON_CYAN, true),
                b'T' | b'B' => (NEON_RED, true),
                b'V' => (VISOR_DIM, true),
                b'L' => (LED_OFF, false),
                _ => continue,
            };
            let open = |ny: usize| matches!(rows[ny][x], b'K' | b'.');
            let shaded = !glow && matches!(c, b'G' | b'g' | b'H' | b'h' | b'J' | b'W');
            // Dark cloth needs a stronger light to show it at all.
            let (light, shade) = match c {
                b'H' | b'J' => (0.3, 0.3),
                b'G' => (0.22, 0.3),
                _ => (0.16, 0.25),
            };
            let color = match () {
                _ if shaded && (y == 0 || open(y - 1)) => mix(color, [0xff, 0xff, 0xff], light),
                _ if shaded && (y + 1 == SIZE || open(y + 1)) => mix(color, [0, 0, 0], shade),
                _ => color,
            };
            let color = match c {
                b'J' | b'g' if !glow && y > 19 => mix(color, [0, 0, 0], 0.045 * (y - 19) as f32),
                _ => color,
            };
            let edge = |nx: usize| matches!(rows[y][nx], b'K' | b'.');
            let color = if matches!(c, b'G' | b'H' | b'J') && x > 0 && edge(x - 1) {
                mix(color, NEON_RED, 0.35)
            } else if matches!(c, b'G' | b'H' | b'J') && x + 1 < SIZE && edge(x + 1) {
                mix(color, NEON_CYAN, 0.35)
            } else {
                color
            };
            px[y][x] = Some(Px { c: color, glow });
        }
    }

    // Long hunger leaves a torn ear.
    if pose.neglect >= 0.5 {
        px[3][8] = None;
        px[4][8] = Some(Px { c: OUTLINE, glow: false });
    }

    // Ear tips, the left one and the right one by turns.
    if !asleep && (41..43).contains(&(pose.frame % 83)) {
        for row in &mut px[3..=4] {
            if (pose.frame / 83).is_multiple_of(2) {
                row.copy_within(6..=10, 5);
                row[10] = None;
            } else {
                row.copy_within(21..=25, 22);
                row[21] = None;
            }
        }
    }

    let mut set = |x: usize, y: usize, c: Rgb, glow: bool| px[y][x] = Some(Px { c, glow });

    let flicker = matches!(pose.activity, Activity::Hungry | Activity::Starving)
        && (pose.frame.wrapping_mul(7919) >> 3).is_multiple_of(5);
    let hiccup = pose.activity == Activity::Stuffed && pose.frame % 35 < 2;
    let empty_paws = matches!(pose.activity, Activity::Hungry | Activity::Starving)
        && pose.morsel.is_none()
        && pose.idle == Idle::Still
        && pose.gait == Gait::Still
        && pose.frame % 70 < 20;
    let scan = match pose.activity {
        _ if pose.stage == 0 => None,
        _ if pose.idle == Idle::Yawn || pose.idle.is_hobby() => None,
        _ if pose.idle == Idle::LookLeft => Some(0),
        _ if pose.idle == Idle::LookRight => Some(12),
        _ if empty_paws => Some(6),
        // A quicker sweep only while chewing a fresh morsel.
        Activity::Eating if pose.morsel.is_some() => Some(pose.frame * 3 / 2),
        Activity::Eating => Some(pose.frame),
        Activity::Content => Some(pose.frame),
        Activity::Hungry | Activity::Starving => Some(pose.frame / 2),
        _ => None,
    };
    let glass = match pose.activity {
        _ if pose.alert && pose.frame % 6 < 3 => VISOR_ALERT,
        _ if pose.petted => VISOR_SCAN,
        Activity::Sleeping => VISOR_OFF,
        _ if flicker => VISOR_OFF,
        Activity::Stuffed => NEON_RED,
        _ => VISOR_DIM,
    };
    let lit = !matches!(glass, VISOR_OFF);
    for x in 9..=22 {
        set(x, 12, glass, lit);
        set(x, 13, glass, lit);
    }
    if lit {
        set(10, 12, mix(glass, [0xff, 0xff, 0xff], 0.55), true);
        set(11, 12, mix(glass, [0xff, 0xff, 0xff], 0.3), true);
    }
    if let Some(t) = scan.filter(|_| !flicker && !pose.alert && !pose.petted) {
        // Ping-pong across the 13 positions of a 2-pixel dot.
        let p = (t % 26) as usize;
        let pos = 9 + if p < 13 { p } else { 25 - p };
        if pose.stage >= 2 {
            let mirror = 30 - pos;
            for x in [pos, pos + 1] {
                set(x, 12, VISOR_SCAN, true);
            }
            for x in [mirror, mirror + 1] {
                set(x, 13, VISOR_SCAN, true);
            }
        } else {
            for x in [pos, pos + 1] {
                set(x, 12, VISOR_SCAN, true);
                set(x, 13, VISOR_SCAN, true);
            }
        }
    }
    if pose.neglect >= 0.8 {
        for (x, y) in [(10, 12), (11, 13), (15, 12), (18, 13), (20, 12)] {
            set(x, y, DIRT, false);
        }
    }

    // Mouth. The base drawing smiles; other moods redraw rows 17 and 18.
    let mut mouth = |top: &[usize], bottom: &[usize], tongue: bool| {
        for x in 13..=18 {
            set(x, 17, MUZZLE, false);
            set(x, 18, MUZZLE, false);
        }
        for &x in top {
            set(x, 17, OUTLINE, false);
        }
        for &x in bottom {
            set(x, 18, OUTLINE, false);
        }
        if tongue {
            set(15, 18, TONGUE, false);
            set(16, 18, TONGUE, false);
        }
    };
    match pose.activity {
        _ if pose.petted => mouth(&[14, 15, 16, 17], &[14, 17], true),
        _ if pose.idle == Idle::Yawn => mouth(&[13, 14, 15, 16, 17, 18], &[13, 14, 17, 18], true),
        _ if pose.idle == Idle::Snack && pose.frame % 6 < 3 => mouth(&[14, 15, 16, 17], &[14, 17], false),
        _ if pose.idle == Idle::Game && pose.frame % 40 < 12 => mouth(&[], &[], true),
        Activity::Eating if pose.morsel.is_some() && pose.frame % 8 < 3 => mouth(&[14, 15, 16, 17], &[14, 17], true),
        Activity::Eating if pose.morsel.is_none() && pose.feast && pose.frame % BITE_FRAMES < 3 => mouth(&[14, 15, 16, 17], &[14, 17], true),
        Activity::Stuffed if hiccup => mouth(&[15, 16], &[15, 16], false),
        Activity::Stuffed => mouth(&[13, 14, 15, 16, 17, 18], &[], false),
        Activity::Starving | Activity::Hungry => mouth(&[14, 15, 16, 17], &[13, 18], false),
        Activity::Bored => mouth(&[14, 15, 16, 17], &[], false),
        Activity::Sleeping => mouth(&[15, 16], &[], false),
        _ => {}
    }

    if let Some(c) = pose.morsel {
        for y in 22..=23 {
            for x in 14..=17 {
                set(x, y, c, true);
            }
            for x in [12, 13, 18, 19] {
                set(x, y, MASK, false);
            }
        }
    }

    if empty_paws {
        for (x, y) in [(13, 22), (14, 23), (15, 23), (16, 23), (17, 23), (18, 22)] {
            set(x, y, MASK, false);
        }
    }

    hobby(&mut set, pose);

    // A pup has no badge, a hacker a dish on the antenna, a legend a cyber arm.
    match pose.stage {
        0 => {
            for (x, y) in [(15, 7), (16, 7), (15, 8), (16, 8)] {
                set(x, y, HAT, false);
            }
        }
        2 | 3 => {
            // A small dish, open to the sky.
            for (x, y) in [(11, 2), (12, 3), (15, 3), (16, 2)] {
                set(x, y, METAL, false);
            }
        }
        _ => {}
    }
    if pose.stage >= 3 {
        for y in 21..=25 {
            set(6, y, CHROME, false);
            set(7, y, CHROME, false);
        }
        // A claw at the chest, and the elbow joint lit.
        set(8, 25, CHROME, false);
        set(9, 25, CHROME, false);
        set(7, 23, NEON_CYAN, true);
    }

    // A pup's antenna is a stub with the LED on top.
    let lit = !asleep && pose.busy && if pose.feast { pose.frame % BITE_FRAMES < 2 } else { pose.frame.is_multiple_of(LED_WINK_FRAMES) };
    let led: &[(usize, usize)] = if pose.stage == 0 { &[(13, 4), (14, 4)] } else { &[(13, 1), (14, 1), (13, 2), (14, 2)] };
    for &(x, y) in led {
        if lit {
            set(x, y, pose.led, true);
        } else {
            set(x, y, LED_OFF, false);
        }
    }
    if pose.stage == 0 {
        for (x, y) in [(13, 1), (14, 1), (13, 2), (14, 2), (13, 3)] {
            px[y][x] = None;
        }
    }

    // The antenna's tip.
    let sway: i32 = match pose.gait {
        _ if pose.stage == 0 => 0,
        Gait::Walking if (pose.frame / 2).is_multiple_of(2) => 1,
        Gait::Walking | Gait::Jumping => -1,
        _ => 0,
    };
    if sway != 0 {
        for row in &mut px[1..=2] {
            let tip = [row[13], row[14]];
            (row[13], row[14]) = (None, None);
            let x = (13 + sway) as usize;
            (row[x], row[x + 1]) = (tip[0], tip[1]);
        }
    }

    if pose.visor != Visor::Down {
        face(&mut px, pose);
    }

    if asleep {
        return curl(&px, pose);
    }
    if matches!(pose.activity, Activity::Hungry | Activity::Starving) || hiccup {
        move_head(&mut px, 0, 1);
    }
    if pose.squash {
        squash(&mut px);
    } else if pose.gait == Gait::Still && pose.idle != Idle::Stretch && (pose.frame / 20) % 2 == 1 {
        stretch(&mut px);
    }
    match pose.idle {
        Idle::Tinker | Idle::Read | Idle::Game => move_head(&mut px, 0, 1),
        Idle::LookLeft => move_head(&mut px, -1, 0),
        Idle::LookRight => move_head(&mut px, 1, 0),
        Idle::Stretch => stretch(&mut px),
        _ => {}
    }
    let hands_free = pose.morsel.is_none() && !empty_paws && !pose.idle.is_hobby();
    match pose.gait {
        Gait::Walking => {
            let phase = (pose.frame / 2) % 4;
            if phase.is_multiple_of(2) {
                stretch(&mut px);
                lift_boot(&mut px, if phase == 0 { 8..=11 } else { 20..=23 });
            }
            if hands_free {
                paws(&mut px, match phase {
                    0 => (24, 26),
                    2 => (26, 24),
                    _ => (25, 25),
                });
            }
        }
        Gait::Jumping => {
            lift_boot(&mut px, 8..=11);
            lift_boot(&mut px, 20..=23);
            if hands_free {
                paws(&mut px, (21, 21));
            }
        }
        Gait::Sitting => sit(&mut px, pose.frame),
        Gait::Still => {
            if hands_free {
                paws(&mut px, (25, 25));
            }
        }
    }
    px
}

const PAW: Rgb = [0xa8, 0xa0, 0xbc];
const PAW_SHADE: Rgb = [0x6a, 0x63, 0x7c];

// Two pixels each: the top rows of the left and the right one.
fn paws(px: &mut Grid, (left, right): (usize, usize)) {
    let (top, under, edge) = (Px { c: PAW, glow: false }, Px { c: PAW_SHADE, glow: false }, Px { c: OUTLINE, glow: false });
    for (x, y) in [(4, left), (26, right)] {
        (px[y][x], px[y][x + 1]) = (Some(top), Some(top));
        (px[y + 1][x], px[y + 1][x + 1]) = (Some(under), Some(under));
    }
    // An outline on the outside of the left one; the right one is in front of the tail.
    (px[left][3], px[left + 1][3]) = (Some(edge), Some(edge));
}

fn hobby(set: &mut impl FnMut(usize, usize, Rgb, bool), pose: &Pose) {
    let f = pose.frame;
    match pose.idle {
        // Soldering a little board, the iron's hot tip spitting sparks.
        Idle::Tinker => {
            for x in 9..=16 {
                set(x, 23, PCB, false);
                set(x, 24, PCB, false);
            }
            for (x, y) in [(10, 23), (12, 24), (14, 23)] {
                set(x, y, CHROME, false);
            }
            for (x, y) in [(22, 19), (21, 20), (20, 21), (19, 22)] {
                set(x, y, CHROME, false);
            }
            set(23, 19, OUTLINE, false);
            set(18, 23, NEON_RED, true);
            if !f.is_multiple_of(3) {
                let (x, y) = [(17, 21), (19, 24), (16, 22), (18, 21)][(f % 4) as usize];
                set(x, y, SPARK, true);
            }
            set(8, 23, MASK, false);
            set(21, 21, MASK, false);
        }
        // The samizdat held up: a red headline, lines of type, a page turning.
        Idle::Read => {
            let turning = f % 60 < 6;
            for y in 20..=26 {
                for x in 9..=22 {
                    set(x, y, if turning && x >= 16 { PAPER_FOLD } else { PAPER }, false);
                }
            }
            for x in 10..=17 {
                set(x, 21, NEON_RED, false);
            }
            for y in [23, 25] {
                for x in 10..=21 {
                    if (x + y) % 4 != 0 {
                        set(x, y, INK, false);
                    }
                }
            }
            set(8, 21, MASK, false);
            set(23, 21, MASK, false);
        }
        // A handheld console, a block falling down its screen.
        Idle::Game => {
            for y in 21..=25 {
                for x in 12..=19 {
                    set(x, y, METAL, false);
                }
            }
            for y in 22..=23 {
                for x in 13..=18 {
                    set(x, y, SCREEN, false);
                }
            }
            set(13 + ((f / 6) % 6) as usize, 22 + ((f / 3) % 2) as usize, PIXEL_GREEN, true);
            set(14, 25, NEON_RED, true);
            set(17, 25, NEON_CYAN, true);
            set(11, 23, MASK, false);
            set(20, 23, MASK, false);
        }
        // A pirozhok in both paws, a bite gone after a while.
        Idle::Snack => {
            let bitten = f % 40 >= 20;
            for (y, from, to, c) in [(21, 14, 17, BUN), (22, 13, 18, BUN), (23, 14, 17, CRUST)] {
                for x in from..=to {
                    if !(bitten && ((x, y) == (17, 21) || (x, y) == (18, 22))) {
                        set(x, y, c, false);
                    }
                }
            }
            set(12, 22, MASK, false);
            set(19, 22, MASK, false);
        }
        _ => {}
    }
}

fn sit(px: &mut Grid, frame: u64) {
    let src = *px;
    px[0] = [None; SIZE];
    px[1..=28].copy_from_slice(&src[..28]);
    for row in &mut px[29..] {
        *row = [None; SIZE];
    }
    let swinging = match (frame / 6) % 4 {
        0 => Some(8),
        2 => Some(20),
        _ => None,
    };
    for leg in [8usize, 20] {
        let up = usize::from(swinging == Some(leg));
        let mut put = |y: usize, x: usize, c: Rgb| px[y - up][x] = Some(Px { c, glow: c == NEON_CYAN });
        put(29, leg, OUTLINE);
        // Tracksuit trousers, the stripe down the outside.
        let stripe = if leg == 8 { leg + 1 } else { leg + 2 };
        for x in leg + 1..=leg + 2 {
            put(29, x, if x == stripe { NEON_CYAN } else { JACKET });
        }
        put(29, leg + 3, OUTLINE);
        put(30, leg - 1, OUTLINE);
        for x in leg..=leg + 3 {
            put(30, x, MASK);
        }
        put(30, leg + 4, OUTLINE);
        for x in leg - 1..=leg + 4 {
            put(31, x, OUTLINE);
        }
    }
}

fn face(px: &mut Grid, pose: &Pose) {
    let mut put = |x: usize, y: usize, c: Rgb| px[y][x] = Some(Px { c, glow: false });
    for x in 8..=23 {
        let blaze = (15..=16).contains(&x);
        put(x, 11, if blaze { MUZZLE } else { FUR });
        for y in 12..=14 {
            put(x, y, if blaze { MUZZLE } else { MASK });
        }
    }
    let closed = pose.activity == Activity::Sleeping || pose.petted || pose.idle == Idle::Yawn || pose.frame % 45 < 2;
    let drowsy = matches!(pose.activity, Activity::Hungry | Activity::Starving | Activity::Bored);
    for eye in [11, 19] {
        if closed {
            put(eye, 13, TRIM);
            put(eye + 1, 13, TRIM);
            continue;
        }
        for x in [eye, eye + 1] {
            if !drowsy {
                put(x, 12, EYE);
            }
            put(x, 13, EYE);
        }
        let pupil = match pose.idle {
            Idle::LookLeft => eye,
            Idle::LookRight => eye + 1,
            _ if eye == 11 => eye + 1,
            _ => eye,
        };
        put(pupil, 13, NOSE);
    }
    let top = if pose.visor == Visor::Half { 10 } else { 9 };
    for y in [top, top + 1] {
        put(8, y, OUTLINE);
        put(23, y, OUTLINE);
        for x in 9..=22 {
            put(x, y, VISOR_DIM);
        }
    }
    if pose.neglect >= 0.8 {
        for x in [10, 15, 20] {
            put(x, top + x % 2, DIRT);
        }
    }
}

fn lift_boot(px: &mut Grid, cols: std::ops::RangeInclusive<usize>) {
    for x in cols {
        for y in 27..=29 {
            px[y][x] = px[y + 1][x];
        }
        px[30][x] = None;
    }
}

// Rows above the body, clear of the tail.
const HEAD_ROWS: std::ops::RangeInclusive<usize> = 1..=18;
const HEAD_COLS: usize = 27;

fn move_head(px: &mut Grid, dx: i32, dy: usize) {
    let src = *px;
    for row in &mut px[HEAD_ROWS] {
        row[..HEAD_COLS].fill(None);
    }
    for y in HEAD_ROWS {
        for (x, p) in src[y][..HEAD_COLS].iter().enumerate() {
            let (Some(p), nx) = (*p, x as i32 + dx) else { continue };
            if (0..HEAD_COLS as i32).contains(&nx) {
                px[y + dy][nx as usize] = Some(p);
            }
        }
    }
}

fn squash(px: &mut Grid) {
    let src = *px;
    px[1..=27].copy_from_slice(&src[..27]);
    px[0] = [None; SIZE];
}

fn stretch(px: &mut Grid) {
    let src = *px;
    px[..27].copy_from_slice(&src[1..28]);
}

fn curl(standing: &Grid, pose: &Pose) -> Grid {
    let mut px: Grid = [[None; SIZE]; SIZE];
    let body = if pose.stage >= 3 { COAT } else { JACKET };
    let mut fill = |y: usize, from: usize, to: usize, c: Rgb| px[y][from..=to].fill(Some(Px { c, glow: false }));
    // A loaf of a body, wider than the head so it shows at the sides.
    for (y, from, to) in [(22, 4, 27), (23, 3, 28), (24, 2, 29), (25, 2, 29), (26, 2, 29), (27, 2, 29), (28, 3, 28)] {
        fill(y, from, to, OUTLINE);
        if y < 28 {
            fill(y, from + 1, to - 1, body);
        }
    }
    // Paws tucked in over the tail, which wraps round the front.
    fill(26, 9, 11, MASK);
    fill(26, 20, 22, MASK);
    for (i, p) in px[27][3..=28].iter_mut().enumerate() {
        let (c, glow) = match i % 6 < 3 {
            true => (NEON_RED, true),
            false if pose.stage >= 2 => (NEON_CYAN, true),
            false => (FUR, false),
        };
        *p = Some(Px { c, glow });
    }
    let dy = 6 + usize::from((pose.frame / 15) % 2 == 1);
    for y in HEAD_ROWS {
        for x in 0..HEAD_COLS {
            if let Some(p) = standing[y][x] {
                px[y + dy][x] = Some(p);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hobby_shows_its_props() {
        let still = draw(&calm(1, 0.0));
        for idle in [Idle::Tinker, Idle::Read, Idle::Game, Idle::Snack] {
            assert_ne!(draw(&Pose { idle, ..calm(1, 0.0) }), still, "{idle:?}");
        }
        assert!(draw(&Pose { idle: Idle::Read, ..calm(1, 0.0) })[22][12].is_some_and(|p| p.c == PAPER));
    }

    #[test]
    fn the_antenna_winks_now_and_then_and_flashes_with_bites() {
        let lit = |pose: Pose| draw(&Pose { led: [1, 2, 3], ..pose }).iter().flatten().any(|p| p.is_some_and(|p| p.c == [1, 2, 3]));
        assert_eq!((0..60).filter(|&frame| lit(Pose { frame, feast: false, ..calm(1, 0.0) })).count(), 2, "a wink every three seconds");
        assert_eq!((0..50).filter(|&frame| lit(Pose { frame, ..calm(1, 0.0) })).count(), 4, "two frames a bite");
        assert!(!lit(Pose { frame: 0, busy: false, ..calm(1, 0.0) }), "dark with no traffic");
    }

    #[test]
    fn rows_are_square() {
        for (y, row) in BASE.iter().enumerate() {
            assert_eq!(row.len(), SIZE, "row {y}");
        }
    }

    fn calm(stage: u8, neglect: f32) -> Pose {
        Pose { activity: Activity::Content, frame: 6, busy: true, feast: true, squash: false, morsel: None, led: NEON_CYAN, alert: false, petted: false, stage, neglect, idle: Idle::Still, gait: Gait::Still, facing_left: false, visor: Visor::Down }
    }

    #[test]
    fn sleeping_curls_up_and_hunger_slumps() {
        let asleep = draw(&Pose { activity: Activity::Sleeping, ..calm(1, 0.0) });
        assert!(asleep[27][3].is_some_and(|p| p.c == NEON_RED), "tail wrapped in front");
        assert!(asleep[25][2].is_some() && asleep[25][29].is_some(), "the body shows at the sides");
        assert!(asleep[1][13].is_none() && asleep[8][13].is_some(), "head lowered onto the body");
        let hungry = draw(&Pose { activity: Activity::Hungry, ..calm(1, 0.0) });
        assert!(hungry[1][13].is_none() && hungry[2][13].is_some(), "slumped a pixel");
        let walking = |frame| draw(&Pose { gait: Gait::Walking, frame, ..calm(1, 0.0) });
        assert!(walking(0)[30][8].is_none() && walking(0)[29][8].is_some() && walking(0)[30][20].is_some(), "left boot up");
        assert!(walking(2)[30][8].is_some() && walking(2)[30][20].is_some(), "both boots down between steps");
        assert!(walking(4)[30][8].is_some() && walking(4)[30][20].is_none(), "then the right");
        let paw = |px: Grid, x: usize, y: usize| px[y][x].is_some_and(|p| p.c == PAW);
        assert!(paw(walking(0), 4, 24) && paw(walking(0), 26, 26), "left paw up, right paw down");
        assert!(paw(walking(4), 4, 26) && paw(walking(4), 26, 24), "then the other way");
        assert!(paw(draw(&calm(1, 0.0)), 4, 25), "paws hang at his sides standing about");
        let sitting = draw(&Pose { gait: Gait::Sitting, frame: 6, ..calm(1, 0.0) });
        assert!(sitting[31][8].is_some() && sitting[31][20].is_some(), "legs over the edge");
        assert!(sitting[1][13].is_none() && sitting[2][13].is_some(), "a pixel lower");
        let homeward = draw(&Pose { activity: Activity::Sleeping, gait: Gait::Walking, ..calm(1, 0.0) });
        assert!(homeward[1][13].is_some(), "on his feet until home");
        let looking = draw(&Pose { idle: Idle::LookLeft, ..calm(1, 0.0) });
        assert_eq!(looking[12][9].map(|p| p.c), Some(VISOR_SCAN), "scanner parked left, head turned");
    }

    #[test]
    fn a_full_belly_hiccups_and_hunger_looks_into_empty_paws() {
        let colours = |pose: &Pose| draw(pose).map(|row| row.map(|p| p.map(|p| p.c)));
        let stuffed = |frame| Pose { activity: Activity::Stuffed, frame, ..calm(1, 0.0) };
        // Frames in the same breath, away from an ear twitch and the tail's wag.
        assert!(colours(&stuffed(0)) != colours(&stuffed(10)), "a hiccup now and then");
        assert!(colours(&stuffed(10)) == colours(&stuffed(12)), "and still in between");
        let hungry = |frame| Pose { activity: Activity::Hungry, frame, ..calm(1, 0.0) };
        assert_eq!(draw(&hungry(0))[23][15].map(|p| p.c), Some(MASK), "paws cupped");
        assert_ne!(draw(&hungry(30))[23][15].map(|p| p.c), Some(MASK), "then down again");
        let walking = Pose { gait: Gait::Walking, ..hungry(0) };
        assert_ne!(draw(&walking)[23][15].map(|p| p.c), Some(MASK), "not while walking");
    }

    #[test]
    fn he_breathes_twitches_his_ears_sways_the_antenna_and_lands_with_a_squash() {
        let still = |frame| Pose { frame, ..calm(1, 0.0) };
        // The antenna's LED is the top of him: it shows how tall he stands.
        assert!(draw(&still(5))[0][13].is_none() && draw(&still(25))[0][13].is_some(), "taller on a breath in");
        assert!(draw(&still(5))[4][6].is_none() && draw(&still(41))[4][6].is_some(), "the left ear bends out");
        assert!(draw(&still(5))[4][25].is_none() && draw(&still(124))[4][25].is_some(), "later the right one");
        let squashed = draw(&Pose { squash: true, ..still(5) });
        assert!(squashed[1][13].is_none() && squashed[2][13].is_some(), "a pixel shorter on landing");
        assert_eq!(squashed[30], draw(&still(5))[30], "feet stay on the ground");
        let walking = |frame| draw(&Pose { gait: Gait::Walking, ..still(frame) });
        assert!(walking(0)[1][15].is_some() && walking(0)[1][13].is_none(), "the tip sways one way");
        assert!(walking(2)[1][12].is_some() && walking(2)[1][14].is_none(), "then the other");
    }

    #[test]
    fn only_a_real_feed_gets_a_bite() {
        let colours = |pose: &Pose| draw(pose).map(|row| row.map(|p| p.map(|p| p.c)));
        let calm = Pose { frame: 0, ..calm(1, 0.0) };
        let trickle = Pose { activity: Activity::Eating, feast: false, ..calm };
        let feast = Pose { activity: Activity::Eating, feast: true, ..calm };
        assert!(colours(&trickle) == colours(&calm), "a trickle leaves his mouth shut");
        assert!(colours(&feast) != colours(&calm), "a real feed is a bite");
    }

    #[test]
    fn stages_add_and_remove_gear() {
        let pup = draw(&calm(0, 0.0));
        let fence = draw(&calm(1, 0.0));
        let legend = draw(&calm(3, 0.0));
        assert_eq!(pup[7][15].map(|p| p.c), Some(HAT), "no badge on a pup");
        assert_eq!(fence[7][15].map(|p| p.c), Some(NEON_RED), "the fence has the badge");
        assert!(pup[1][13].is_none() && pup[4][13].is_some(), "a pup's antenna is a stub");
        assert_eq!(legend[2][11].map(|p| p.c), Some(METAL), "dish");
        assert_eq!(legend[25][9].map(|p| p.c), Some(CHROME), "claw");
        assert_eq!(legend[23][7].map(|p| p.c), Some(NEON_CYAN), "cyber arm");
    }

    #[test]
    fn visor_up_shows_the_raccoon() {
        let up = draw(&Pose { visor: Visor::Up, ..calm(1, 0.0) });
        assert_eq!(up[9][15].map(|p| p.c), Some(VISOR_DIM), "visor parked on the hat");
        assert_eq!(up[12][11].map(|p| p.c), Some(EYE), "eyes in the mask");
        assert_eq!(up[13][12].map(|p| p.c), Some(NOSE), "a pupil");
        assert_eq!(up[13][15].map(|p| p.c), Some(MUZZLE), "the blaze between the eyes");
        let half = draw(&Pose { visor: Visor::Half, ..calm(1, 0.0) });
        assert_eq!(half[11][15].map(|p| p.c), Some(VISOR_DIM), "on its way up");
        let asleep = draw(&Pose { visor: Visor::Up, activity: Activity::Sleeping, ..calm(1, 0.0) });
        assert!(!asleep.iter().flatten().any(|p| p.is_some_and(|p| p.c == EYE)), "eyes closed in sleep");
    }

    #[test]
    fn neglect_shows_after_long_hunger() {
        assert!(draw(&calm(1, 0.0))[3][8].is_some());
        assert!(draw(&calm(1, 0.6))[3][8].is_none(), "torn ear");
        assert_eq!(draw(&calm(1, 0.9))[12][10].map(|p| p.c), Some(DIRT), "grimy visor");
    }

    #[test]
    #[ignore]
    fn write_stage_sheet() {
        let out = std::env::temp_dir().join("raccy-stages.bmp");
        std::fs::write(&out, stage_sheet(8)).unwrap();
        println!("{}", out.display());
    }

    #[test]
    fn the_stages_draw_as_their_golden_sheet() {
        crate::render::golden::check("stages", &stage_sheet(2));
    }

    fn stage_sheet(scale: usize) -> Vec<u8> {
        let poses = [
            calm(0, 0.0),
            calm(1, 0.0),
            calm(2, 0.0),
            calm(3, 0.0),
            calm(1, 1.0),
            Pose { activity: Activity::Sleeping, ..calm(1, 0.0) },
            Pose { activity: Activity::Hungry, ..calm(1, 0.0) },
            Pose { idle: Idle::LookLeft, ..calm(1, 0.0) },
            Pose { idle: Idle::Yawn, ..calm(1, 0.0) },
            Pose { idle: Idle::Stretch, ..calm(1, 0.0) },
            Pose { gait: Gait::Jumping, ..calm(1, 0.0) },
            Pose { gait: Gait::Sitting, frame: 6, ..calm(1, 0.0) },
            Pose { gait: Gait::Sitting, frame: 0, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, ..calm(0, 0.0) },
            Pose { visor: Visor::Up, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, ..calm(3, 0.0) },
            Pose { visor: Visor::Half, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, activity: Activity::Sleeping, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, activity: Activity::Hungry, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, idle: Idle::LookLeft, ..calm(1, 0.0) },
            Pose { visor: Visor::Up, petted: true, ..calm(1, 0.0) },
        ];
        let (w, h) = (poses.len() * (SIZE + 4) * scale, (SIZE + 4) * scale);
        let mut rgb = vec![[0x28u8, 0x24, 0x30]; w * h];
        for (i, pose) in poses.iter().enumerate() {
            let px = draw(pose);
            for (y, row) in px.iter().enumerate() {
                for (x, p) in row.iter().enumerate() {
                    let Some(p) = p else { continue };
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (sx, sy) = ((i * (SIZE + 4) + 2 + x) * scale + dx, (2 + y) * scale + dy);
                            rgb[sy * w + sx] = p.c;
                        }
                    }
                }
            }
        }
        crate::render::golden::bmp(w, h, |x, y| rgb[y * w + x])
    }

    #[test]
    fn every_pose_draws() {
        for activity in [
            Activity::Sleeping,
            Activity::Stuffed,
            Activity::Eating,
            Activity::Starving,
            Activity::Hungry,
            Activity::Bored,
            Activity::Content,
        ] {
            for frame in 0..60 {
                let pose = Pose { activity, frame, busy: true, feast: true, squash: frame % 7 == 0, morsel: Some([1, 2, 3]), led: [9, 9, 9], alert: frame % 2 == 0, petted: frame % 3 == 0, stage: (frame % 4) as u8, neglect: (frame % 5) as f32 / 4.0, idle: [Idle::Still, Idle::LookLeft, Idle::LookRight, Idle::Yawn, Idle::Stretch][frame as usize % 5], gait: [Gait::Still, Gait::Walking, Gait::Jumping, Gait::Sitting][frame as usize % 4], facing_left: frame % 2 == 0, visor: [Visor::Down, Visor::Half, Visor::Up][frame as usize % 3] };
                let px = draw(&pose);
                assert!(px[12][9].is_some(), "{activity:?} visor");
            }
        }
    }
}

