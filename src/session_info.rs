//! iRacing SessionInfo YAML parser and driver roster.
//!
//! iRacing exposes a `SessionInfo` string variable containing a YAML blob with
//! the full entry list: car number, driver name, team name, car class, etc.
//! A monotonically-increasing `SessionInfoUpdate` counter in the telemetry stream
//! signals when the blob has changed (e.g. a driver disconnects or reconnects).
//!
//! # Usage
//!
//! ```no_run
//! use director_narrative_core::session_info::{SessionInfoParser, RosterCache};
//!
//! let mut cache = RosterCache::new();
//!
//! // Called each frame with the current update tick and a closure that
//! // provides the raw YAML string only when the cache is stale.
//! let session_info_update: u32 = 3; // from TelemetryFrame or mmap header
//! if cache.needs_update(session_info_update) {
//!     let yaml = "<yaml from mmap>";
//!     cache.update(session_info_update, yaml).ok();
//! }
//!
//! if let Some(car) = cache.roster().and_then(|r| r.lookup(42)) {
//!     println!("{} — {}", car.car_number, car.driver_name);
//! }
//! ```

use std::collections::HashMap;

use serde::Serialize;

// ── Public types ─────────────────────────────────────────────────────────────

/// Resolved identity for a single car slot in the iRacing entry list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CarRef {
    pub car_idx: u8,
    /// iRacing car number string (e.g. `"42"`, `"042"`, `"P1"`).
    pub car_number: String,
    /// Driver display name as returned by iRacing (`UserName`).
    pub driver_name: String,
    pub team_name: Option<String>,
    pub car_class_short_name: Option<String>,
    pub car_class_id: Option<u32>,
    /// iRacing CustID — used by Race Control for identity resolution.
    pub user_id: Option<u32>,
}

/// Immutable roster built from one parse of the `SessionInfo` YAML.
pub struct SessionRoster {
    cars: HashMap<u8, CarRef>,
}

impl SessionRoster {
    /// Look up a car by its `carIdx` slot number.
    pub fn lookup(&self, car_idx: u8) -> Option<&CarRef> {
        self.cars.get(&car_idx)
    }

    /// Number of car slots in the roster (includes inactive pacecar slots).
    pub fn len(&self) -> usize {
        self.cars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cars.is_empty()
    }
}

/// Stateless YAML parser. Call [`SessionInfoParser::build`] whenever the
/// `SessionInfoUpdate` counter changes.
pub struct SessionInfoParser;

impl SessionInfoParser {
    /// Parse a raw `SessionInfo` YAML string into a [`SessionRoster`].
    ///
    /// Returns an error if the YAML is malformed or the required
    /// `DriverInfo.Drivers` key is missing.
    pub fn build(yaml_str: &str) -> Result<SessionRoster, serde_yaml::Error> {
        let root: YamlRoot = serde_yaml::from_str(yaml_str)?;
        let cars = root
            .driver_info
            .drivers
            .into_iter()
            .map(|d| {
                let car_ref = CarRef {
                    car_idx: d.car_idx,
                    car_number: d.car_number,
                    driver_name: d.user_name,
                    team_name: non_empty(d.team_name),
                    car_class_short_name: non_empty(d.car_class_short_name),
                    car_class_id: d.car_class_id,
                    user_id: d.user_id,
                };
                (d.car_idx, car_ref)
            })
            .collect();
        Ok(SessionRoster { cars })
    }
}

// ── RosterCache ───────────────────────────────────────────────────────────────

/// Wraps a [`SessionRoster`] and tracks the last-parsed `SessionInfoUpdate`
/// tick so callers can detect when a re-parse is needed.
///
/// The cache starts empty (no roster). Call [`needs_update`] each frame;
/// when it returns `true`, read the YAML string from the mmap and call
/// [`update`].
///
/// [`needs_update`]: RosterCache::needs_update
/// [`update`]: RosterCache::update
pub struct RosterCache {
    last_tick: Option<u32>,
    roster: Option<SessionRoster>,
}

impl RosterCache {
    pub fn new() -> Self {
        Self {
            last_tick: None,
            roster: None,
        }
    }

    /// Returns `true` if `current_tick` differs from the last tick this cache
    /// was updated at, or if the cache has never been populated.
    pub fn needs_update(&self, current_tick: u32) -> bool {
        self.last_tick != Some(current_tick)
    }

    /// Parse `yaml_str` and store the resulting roster tagged with `tick`.
    ///
    /// Replaces the previous roster on success. On parse failure the previous
    /// roster (if any) is retained and the tick is **not** advanced, so the
    /// next frame will retry.
    pub fn update(&mut self, tick: u32, yaml_str: &str) -> Result<(), serde_yaml::Error> {
        let roster = SessionInfoParser::build(yaml_str)?;
        self.roster = Some(roster);
        self.last_tick = Some(tick);
        Ok(())
    }

    /// Returns the current roster, or `None` if the cache has never been
    /// successfully populated.
    pub fn roster(&self) -> Option<&SessionRoster> {
        self.roster.as_ref()
    }
}

impl Default for RosterCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Private serde types ───────────────────────────────────────────────────────

/// Normalise iRacing's empty-string sentinels to `None`.
fn non_empty(s: Option<String>) -> Option<String> {
    s.and_then(|v| if v.trim().is_empty() { None } else { Some(v) })
}

#[derive(serde::Deserialize)]
struct YamlRoot {
    #[serde(rename = "DriverInfo")]
    driver_info: YamlDriverInfo,
}

#[derive(serde::Deserialize)]
struct YamlDriverInfo {
    #[serde(rename = "Drivers")]
    drivers: Vec<YamlDriver>,
}

#[derive(serde::Deserialize)]
struct YamlDriver {
    #[serde(rename = "CarIdx")]
    car_idx: u8,
    #[serde(rename = "UserName")]
    user_name: String,
    #[serde(rename = "UserID", default)]
    user_id: Option<u32>,
    #[serde(rename = "TeamName", default)]
    team_name: Option<String>,
    #[serde(rename = "CarNumber")]
    car_number: String,
    #[serde(rename = "CarClassID", default)]
    car_class_id: Option<u32>,
    #[serde(rename = "CarClassShortName", default)]
    car_class_short_name: Option<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal SessionInfo YAML covering 4 drivers with varying optional fields.
    const FIXTURE_YAML: &str = r#"
DriverInfo:
 DriverCarIdx: 0
 Drivers:
 - CarIdx: 0
   UserName: Paul Crofts
   UserID: 123456
   TeamName: Team Margic
   CarNumber: "42"
   CarClassID: 4011
   CarClassShortName: GTP
 - CarIdx: 1
   UserName: Alice Racer
   UserID: 234567
   TeamName: ""
   CarNumber: "7"
   CarClassID: 4011
   CarClassShortName: GTP
 - CarIdx: 2
   UserName: Bob Speedman
   CarNumber: "18"
   CarClassID: 2523
   CarClassShortName: LMP2
 - CarIdx: 63
   UserName: Pace Car
   UserID: 0
   CarNumber: "0"
   CarClassID: 11
   CarClassShortName: Pace
"#;

    #[test]
    fn parse_returns_all_drivers() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse should succeed");
        assert_eq!(roster.len(), 4);
    }

    #[test]
    fn lookup_known_car_returns_correct_ref() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse");
        let car = roster.lookup(0).expect("carIdx 0 should exist");
        assert_eq!(car.car_number, "42");
        assert_eq!(car.driver_name, "Paul Crofts");
        assert_eq!(car.team_name, Some("Team Margic".to_string()));
        assert_eq!(car.car_class_short_name, Some("GTP".to_string()));
        assert_eq!(car.car_class_id, Some(4011));
        assert_eq!(car.user_id, Some(123456));
    }

    #[test]
    fn lookup_unknown_car_returns_none() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse");
        assert!(roster.lookup(99).is_none());
    }

    #[test]
    fn empty_team_name_normalised_to_none() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse");
        let car = roster.lookup(1).expect("carIdx 1 should exist");
        // TeamName was "" — should be None after normalisation
        assert_eq!(car.team_name, None);
    }

    #[test]
    fn absent_optional_fields_are_none() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse");
        let car = roster.lookup(2).expect("carIdx 2 should exist");
        // carIdx 2 has no TeamName or UserID keys at all
        assert_eq!(car.team_name, None);
        assert_eq!(car.user_id, None);
    }

    #[test]
    fn high_car_idx_slot_parsed_correctly() {
        let roster = SessionInfoParser::build(FIXTURE_YAML).expect("parse");
        let car = roster.lookup(63).expect("carIdx 63 (pace car) should exist");
        assert_eq!(car.car_number, "0");
        assert_eq!(car.driver_name, "Pace Car");
    }

    #[test]
    fn roster_cache_starts_needing_update() {
        let cache = RosterCache::new();
        assert!(cache.needs_update(1));
        assert!(cache.roster().is_none());
    }

    #[test]
    fn roster_cache_satisfied_after_update() {
        let mut cache = RosterCache::new();
        cache.update(5, FIXTURE_YAML).expect("update should succeed");
        assert!(!cache.needs_update(5));
        assert!(cache.needs_update(6));
    }

    #[test]
    fn roster_cache_retains_roster_on_bad_yaml() {
        let mut cache = RosterCache::new();
        cache.update(1, FIXTURE_YAML).expect("first update");
        // Simulate a corrupted re-read — the parse fails
        let result = cache.update(2, "not: valid: yaml: at: all: {{");
        assert!(result.is_err());
        // Previous roster is still accessible and tick has NOT advanced
        assert!(cache.needs_update(2));
        assert!(cache.roster().is_some());
    }

    #[test]
    fn roster_cache_lookup_via_cache() {
        let mut cache = RosterCache::new();
        cache.update(3, FIXTURE_YAML).expect("update");
        let car = cache
            .roster()
            .and_then(|r| r.lookup(0))
            .expect("should find carIdx 0");
        assert_eq!(car.driver_name, "Paul Crofts");
    }
}
