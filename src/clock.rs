use std::time::{SystemTime, UNIX_EPOCH};

use crate::tuning::tuning;

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn offset_at(unix: i64) -> i64 {
    crate::platform::host::tz_offset_at(unix)
}

fn local(unix: u64) -> i64 {
    unix as i64 + offset_at(unix as i64)
}

pub fn local_clock() -> (u8, u32) {
    let local = local(unix_now());
    (local.rem_euclid(86_400).div_euclid(3600) as u8, local.div_euclid(86_400) as u32)
}

pub fn clock_text(unix: u64) -> String {
    let secs = local(unix).rem_euclid(86_400);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

pub fn stamp(unix: u64) -> String {
    let local = local(unix);
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = local.div_euclid(86_400) + 719_468;
    let doe = z - z.div_euclid(146_097) * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{day:02}.{month:02}. {}", clock_text(unix))
}

pub fn record_stamp(unix: u64) -> String {
    format!("{}:{:02}", stamp(unix), local(unix).rem_euclid(60))
}

pub fn next_morning() -> u64 {
    morning_after(unix_now() as i64)
}

fn morning_after(now: i64) -> u64 {
    let today = now + offset_at(now);
    let mut target = today - today.rem_euclid(86_400) + tuning().mute.morning_hour * 3600;
    if target <= today {
        target += 86_400;
    }
    // The offset at seven in the morning, which summer time starting or
    // ending overnight makes different from now's.
    let guess = target - offset_at(now);
    (target - offset_at(guess)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_a_clock_does() {
        assert_eq!(stamp(1_757_930_000).len(), 12);
        assert!(offset_at(1_757_930_000).abs() <= 14 * 3600);
        let morning = next_morning();
        assert!(morning > unix_now() && morning <= unix_now() + 86_400 + 3600);
        assert_eq!(clock_text(morning), "07:00");
    }

    // Checked only where this machine keeps Central European time.
    #[test]
    fn summer_time_starts_and_ends_on_its_dates() {
        if offset_at(1_768_478_400) != 3600 || offset_at(1_784_116_800) != 7200 {
            return;
        }
        // 29 March 2026, 01:00 UTC, and 25 October 2026, 01:00 UTC.
        assert_eq!(offset_at(1_774_745_999), 3600, "the last second of winter time");
        assert_eq!(offset_at(1_774_746_000), 7200, "summer time from three in the morning");
        assert_eq!(clock_text(1_774_746_000), "03:00");
        assert_eq!(offset_at(1_792_889_999), 7200);
        assert_eq!(offset_at(1_792_890_000), 3600, "back to winter time");
        assert_eq!(stamp(1_792_889_999), "25.10. 02:59");
        assert_eq!(clock_text(1_792_890_000), "02:00");
        assert_eq!(morning_after(1_774_735_200), 1_774_760_400, "spring: 05:00 UTC");
        assert_eq!(morning_after(1_792_879_200), 1_792_908_000, "autumn: 06:00 UTC");
    }
}
