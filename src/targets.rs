//! `trace_targets.txt`: which classes/methods to hook.

use std::fs;
use std::path::Path;

pub const FILE_NAME: &str = "trace_targets.txt";

pub const DEFAULT_FILE: &str = "\
# uma-esport-spectator: trace targets
#
# One pattern per line, matched against classes in umamusume.dll.
#   Namespace.Class          hook every method of that class
#   Namespace.Prefix*        `*` matches anything
#   Outer+Inner              nested classes use `+`
#   !pattern                 exclude; matched against `Namespace.Class::Method`
#
# Changes are read when hooks are installed (first time recording is turned
# on), so restart the game after editing.

Gallop.RaceManager*
Gallop.RaceHorseManager*
Gallop.HorseRaceInfo*
Gallop.RaceSimulate*
Gallop.RaceEventPlayer*
Gallop.RacePhaseCalculator*
Gallop.RaceInfo
Gallop.RaceViewBase
Gallop.RaceViewReplay
Gallop.RaceMainViewController
Gallop.SkillManager
Gallop.SkillBase

# Noise: per-frame callbacks and property accessors.
!*::Update
!*::LateUpdate
!*::FixedUpdate
!*::get_*
!*::set_*
";

#[derive(Default, Debug)]
pub struct Targets {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Targets {
    /// Loads the file, creating it with defaults if missing.
    pub fn load_or_create(path: &Path) -> Targets {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                let _ = fs::write(path, DEFAULT_FILE);
                DEFAULT_FILE.to_owned()
            }
        };
        Targets::parse(&text)
    }

    pub fn parse(text: &str) -> Targets {
        let mut t = Targets::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match line.strip_prefix('!') {
                Some(p) => t.exclude.push(p.trim().to_owned()),
                None => t.include.push(line.to_owned()),
            }
        }
        t
    }

    pub fn wants_class(&self, class: &str) -> bool {
        self.include.iter().any(|p| wildcard(p, class))
    }

    /// `full` is `Namespace.Class::Method`.
    pub fn wants_method(&self, full: &str) -> bool {
        !self.exclude.iter().any(|p| wildcard(p, full))
    }
}

/// Glob match where `*` matches any run of characters (case-sensitive).
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching() {
        assert!(wildcard("Gallop.Race*", "Gallop.RaceManager"));
        assert!(wildcard("Gallop.RaceInfo", "Gallop.RaceInfo"));
        assert!(!wildcard("Gallop.RaceInfo", "Gallop.RaceInfoReplay"));
        assert!(wildcard("*::get_*", "Gallop.RaceManager::get_Instance"));
        assert!(!wildcard("*::Update", "Gallop.RaceManager::UpdateRace"));
        assert!(wildcard("*", ""));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
    }

    #[test]
    fn default_file_parses() {
        let t = Targets::parse(DEFAULT_FILE);
        assert!(t.wants_class("Gallop.RaceManager"));
        assert!(t.wants_class("Gallop.HorseRaceInfoReplay"));
        assert!(!t.wants_class("Gallop.HomeViewController"));
        assert!(t.wants_method("Gallop.RaceManager::StartRace"));
        assert!(!t.wants_method("Gallop.RaceManager::Update"));
        assert!(!t.wants_method("Gallop.RaceManager::get_Instance"));
    }
}
