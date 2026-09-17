use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::net::services::Service;

pub const SOURCE: &str = include_str!("../detect.toml");

// The file is part of the build and cargo test reads it, so it cannot fail to read in a build that passed its tests.
pub fn detect() -> &'static Detect {
    static DETECT: OnceLock<Detect> = OnceLock::new();
    DETECT.get_or_init(|| toml::from_str(SOURCE).unwrap_or_else(|e| panic!("detect.toml: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    pub private_words: Vec<String>,
    pub calls_home_words: Vec<String>,
    pub plain_http: PlainHttp,
    pub watch: Watch,
    pub seat: HashMap<SeatKind, Vec<String>>,
    pub service: Vec<Known>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
pub enum SeatKind {
    Browser,
    Code,
    Terminal,
    Chat,
    Mail,
    Office,
    Media,
    Games,
    Files,
    Design,
}

impl SeatKind {
    #[cfg(test)]
    pub const ALL: [SeatKind; 10] = [
        SeatKind::Browser,
        SeatKind::Code,
        SeatKind::Terminal,
        SeatKind::Chat,
        SeatKind::Mail,
        SeatKind::Office,
        SeatKind::Media,
        SeatKind::Games,
        SeatKind::Files,
        SeatKind::Design,
    ];
}

pub fn seat_kind(process: &str) -> Option<SeatKind> {
    let process = process.to_lowercase();
    detect().seat.iter().find(|(_, programs)| programs.contains(&process)).map(|(kind, _)| *kind)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Known {
    pub name: Service,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub processes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlainHttp {
    pub labels: Vec<String>,
    pub suffixes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub system_tools: Vec<String>,
    pub shells: Vec<String>,
    pub cheap_tlds: Vec<String>,
    pub resolvers: Vec<String>,
    pub system_itself: Vec<String>,
    pub mining_ports: Vec<u16>,
    pub mining_words: Vec<String>,
    pub tor_ports: Vec<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_program_is_one_kind_of_seat_at_most() {
        let mut seen = HashSet::new();
        for programs in detect().seat.values() {
            for program in programs {
                assert_eq!(program, &program.to_lowercase(), "programs are lowercase");
                assert!(seen.insert(program.clone()), "{program} is of two kinds");
            }
        }
        assert_eq!(seat_kind("Code"), Some(SeatKind::Code));
        assert_eq!(seat_kind("notanapp"), None);
    }

    #[test]
    fn the_detection_file_reads_and_nothing_belongs_to_two_services() {
        if let Err(e) = toml::from_str::<Detect>(SOURCE) {
            panic!("detect.toml: {e}");
        }
        let (mut names, mut domains, mut programs) = (HashSet::new(), HashSet::new(), HashSet::new());
        for known in &detect().service {
            assert!(names.insert(format!("{:?}", known.name)), "{:?} listed twice", known.name);
            assert!(!known.domains.is_empty() || !known.processes.is_empty(), "{:?} has nothing to know it by", known.name);
            for domain in &known.domains {
                assert_eq!(domain, &domain.to_lowercase(), "domains are lowercase");
                assert!(domains.insert(domain.clone()), "{domain} belongs to two services");
            }
            for program in &known.processes {
                assert_eq!(program, &program.to_lowercase(), "programs are lowercase");
                assert!(programs.insert(program.clone()), "{program} belongs to two services");
            }
        }
    }
}
