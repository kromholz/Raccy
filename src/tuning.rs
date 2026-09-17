use std::sync::OnceLock;

use serde::Deserialize;

pub const SOURCE: &str = include_str!("../tuning.toml");

// The file is part of the build and cargo test reads it, so it cannot fail to read in a build that passed its tests.
pub fn tuning() -> &'static Tuning {
    static TUNING: OnceLock<Tuning> = OnceLock::new();
    TUNING.get_or_init(|| toml::from_str(SOURCE).unwrap_or_else(|e| panic!("tuning.toml: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tuning {
    pub pet: Pet,
    pub life: Life,
    pub mute: Mute,
    pub talk: Talk,
    pub watch: Watch,
    pub lan: Lan,
    pub roam: Roam,
    pub scene: Scene,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pet {
    pub food_line_bps: u64,
    pub food_per_doubling: f32,
    pub quiet_line_bps: u64,
    pub asleep_after_secs: u32,
    pub hours_to_empty: f32,
    pub night_cost_percent: f32,
    pub hours_to_bored: f32,
    pub novelty_secs: u64,
    pub stuffed_bps: u64,
    pub offline_satiety_floor: f32,
    pub offline_mood_floor: f32,
    pub learning_secs: u64,
    pub regular_days: u32,
    pub stage_days: [u32; 3],
    pub fed_day_secs: u32,
    pub fed_satiety: f32,
    pub starving_days_to_tear: f32,
    pub fed_days_to_mend: f32,
}

impl Pet {
    pub fn hunger_per_sec(&self) -> f32 {
        100.0 / (self.hours_to_empty * 3600.0)
    }

    pub fn hunger_asleep_per_sec(&self) -> f32 {
        self.night_cost_percent / (8.0 * 3600.0)
    }

    pub fn boredom_per_sec(&self) -> f32 {
        100.0 / (self.hours_to_bored * 3600.0)
    }

    pub fn neglect_per_starving_sec(&self) -> f32 {
        1.0 / (self.starving_days_to_tear * 86_400.0)
    }

    pub fn mending_per_fed_sec(&self) -> f32 {
        1.0 / (self.fed_days_to_mend * 86_400.0)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Life {
    pub idle_every_frames: [u32; 2],
    pub idle_mix: IdleMix,
    pub nap_min_secs: u64,
    pub nap_max_secs: u64,
    pub nap_idle_day_secs: u64,
    pub nap_one_in_day_secs: u64,
    pub nap_idle_night_secs: u64,
    pub nap_one_in_night_secs: u64,
    pub night_from_hour: u8,
    pub night_until_hour: u8,
    pub visor_up_frames: (u64, u64),
    pub visor_down_frames: (u64, u64),
    pub petted_frames: u32,
    pub stroke_window_frames: u64,
}

impl Life {
    pub fn is_night(&self, hour: u8) -> bool {
        !(self.night_until_hour..self.night_from_hour).contains(&hour)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mute {
    pub morning_hour: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Talk {
    pub line_secs: u32,
    pub warn_secs: u32,
    pub cooldown_secs: u64,
    pub warn_gap_secs: u64,
    pub name_wait_secs: u32,
    pub spike_bps: u64,
    pub spike_every_secs: u64,
    pub spike_cause_secs: u64,
    pub owner_share: f64,
    pub appetite_secs: f64,
    pub feast_bps: u64,
    pub big_feast_bps: u64,
    pub trickle_bps: u64,
    pub chatter_big_feast_secs: u64,
    pub chatter_feast_secs: u64,
    pub chatter_trickle_secs: u64,
    pub chatter_quiet_secs: u64,
    pub feast_hold_secs: u64,
    pub feast_every_secs: u64,
    pub repeat_every_secs: u64,
    pub service_every_secs: u64,
    pub subject_quiet_secs: u64,
    pub warn_repeat_secs: u64,
    pub note_repeat_secs: u64,
    pub exposed_remind_secs: u64,
    pub exposed_note_remind_secs: u64,
    pub note_stale_secs: u64,
    pub warn_stale_secs: u64,
    pub hold_idle_secs: u64,
    pub moan_every_secs: u64,
    pub summary_every_secs: u64,
    pub lore_every_secs: u64,
    pub lore_quiet_secs: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub beacon_opens: usize,
    pub beacon_min_secs: f64,
    pub beacon_max_secs: f64,
    pub beacon_jitter: f64,
    pub sweep_window_secs: u64,
    pub sweep_hosts: usize,
    pub sweep_ports: usize,
    pub upload_bps: u64,
    pub upload_secs: u32,
    pub upload_again_secs: u32,
    pub owner_share: f64,
    pub away_secs: u64,
    pub name_wait_secs: u64,
    pub session_confirm_secs: u64,
    pub session_grace_secs: u64,
    pub offline_after_secs: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lan {
    pub look_every_secs: u64,
    pub learning_secs: u64,
    pub new_gateway_looks: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roam {
    pub patrol_one_in_frames: u32,
    pub sit_one_in_frames: u32,
    pub travel_one_in_frames: u32,
    pub patrol_min_px: i32,
    pub patrol_max_px: i32,
    pub pause_frames: u32,
    pub sit_min_frames: u32,
    pub sit_max_frames: u32,
    pub sit_wish_frames: u32,
    pub settle_frames: u32,
    pub announce_frames: u32,
    pub walk_step_px: i32,
    pub jump_step_px: i32,
    pub jump_min_frames: i32,
    pub jump_max_frames: i32,
    pub jump_height_px: i32,
    pub climb_step_px: u32,
    pub sink_step_px: i32,
    pub sink_min_frames: i32,
    pub sink_max_frames: i32,
}

// Shares of each kind of idle moment.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdleMix {
    pub look_left: u32,
    pub look_right: u32,
    pub yawn: u32,
    pub stretch: u32,
    pub tinker: u32,
    pub read: u32,
    pub game: u32,
    pub snack: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub big_bits_bps: u64,
    pub glitch_bps: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tuning_file_reads_and_makes_sense() {
        if let Err(e) = toml::from_str::<Tuning>(SOURCE) {
            panic!("tuning.toml: {e}");
        }
        let t = tuning();
        assert!(t.life.nap_min_secs < t.life.nap_max_secs, "naps");
        assert!(t.life.visor_up_frames.0 < t.life.visor_up_frames.1 && t.life.visor_down_frames.0 < t.life.visor_down_frames.1, "visor");
        assert!(t.life.night_until_hour < t.life.night_from_hour && t.life.night_from_hour < 24, "night");
        assert!(t.roam.patrol_min_px < t.roam.patrol_max_px && t.roam.sit_min_frames < t.roam.sit_max_frames, "roaming");
        assert!(t.roam.jump_min_frames <= t.roam.jump_max_frames && t.roam.sink_min_frames <= t.roam.sink_max_frames, "jumps");
        // Every one of these is divided by somewhere, or counts the frames a
        // movement is cut into. A zero would be a division by nothing rather
        // than a pet that stands still.
        let steps = [t.roam.walk_step_px, t.roam.jump_step_px, t.roam.sink_step_px, t.roam.climb_step_px as i32, t.roam.jump_height_px];
        assert!(steps.iter().all(|&px| px > 0), "every step is a step: {steps:?}");
        let frames = [t.roam.jump_min_frames, t.roam.sink_min_frames];
        assert!(frames.iter().all(|&n| n > 0), "a movement lasts at least a frame: {frames:?}");
        assert!(t.pet.stage_days.windows(2).all(|w| w[0] < w[1]), "stages grow");
        assert!(t.talk.trickle_bps < t.talk.feast_bps && t.talk.feast_bps < t.talk.big_feast_bps, "appetite");
        assert!((0..24).contains(&t.mute.morning_hour), "morning");
        assert!(t.pet.quiet_line_bps < t.pet.food_line_bps, "food above quiet");
    }
}
