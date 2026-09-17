use crate::tuning::tuning;
use crate::watch::Finding;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    Internet,
    // A network, but nothing past it, or only a login page in the way.
    Local,
    Nothing,
    Unknown,
}

#[derive(Default)]
pub struct Wire {
    down_since: Option<u64>,
    told: Option<u64>,
}

impl Wire {
    // One look a second.
    pub fn tick(&mut self, now: u64, reach: Reach) -> Option<Finding> {
        match reach {
            Reach::Unknown => None,
            Reach::Internet => {
                self.down_since = None;
                let started = self.told.take()?;
                Some(Finding::Online { started, secs: now.saturating_sub(started) })
            }
            Reach::Local | Reach::Nothing => {
                let since = *self.down_since.get_or_insert(now);
                (self.told.is_none() && now.saturating_sub(since) >= tuning().watch.offline_after_secs).then(|| {
                    self.told = Some(since);
                    Finding::Offline { started: since, local: reach == Reach::Local }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outage_is_told_once_it_lasts_and_its_end_with_how_long() {
        let after = tuning().watch.offline_after_secs;
        let mut wire = Wire::default();
        assert_eq!(wire.tick(100, Reach::Internet), None);
        assert_eq!(wire.tick(101, Reach::Local), None, "a blip is no news");
        assert_eq!(wire.tick(100 + after, Reach::Local), None);
        assert_eq!(wire.tick(101 + after, Reach::Local), Some(Finding::Offline { started: 101, local: true }));
        assert_eq!(wire.tick(110 + after, Reach::Nothing), None, "told once");
        assert_eq!(wire.tick(300, Reach::Unknown), None, "not knowing changes nothing");
        assert_eq!(wire.tick(301, Reach::Internet), Some(Finding::Online { started: 101, secs: 200 }));
        assert_eq!(wire.tick(302, Reach::Internet), None);
        assert_eq!(wire.tick(400, Reach::Nothing), None);
        assert_eq!(wire.tick(402, Reach::Internet), None);
    }

    #[test]
    #[ignore]
    fn reach_live() {
        println!("{:?}", crate::platform::wire::reach());
    }
}
