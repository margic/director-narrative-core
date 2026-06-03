use serde::Serialize;

use crate::battle_state::SlopeInfo;

/// Classification of a yellow flag's scope relative to the player.
#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FlagScope {
    /// The player's own actions caused the yellow condition.
    SelfCaused,
    /// Another car caused the yellow, but in close proximity to the player.
    Nearby,
    /// A caution or yellow condition that affects all cars session-wide.
    SessionWide,
    /// Unable to determine the scope from available data.
    Unknown,
}

/// All narrative events emitted by the engine.
#[derive(Debug, Serialize)]
#[serde(tag = "event_type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RaceEvent {
    BattleEngaged {
        lap:                               u8,
        session_time:                      f32,
        player_car_idx:                    u8,
        opponent_car_idx:                  u8,
        gap_s:                             f32,
        car_race_position:                 u8,
        prior_skirmishes:                  u32,
        prior_attack_time_s:               f32,
        engagement_started_at_session_time_s: f32,
    },
    BattleBroken {
        lap:                               u8,
        session_time:                      f32,
        player_car_idx:                    u8,
        opponent_car_idx:                  u8,
        final_gap_sec:                     Option<f32>,
        car_race_position:                 u8,
        engagement_started_at_session_time_s: f32,
    },
    BattleClosing {
        lap:                      u8,
        session_time:             f32,
        player_car_idx:           u8,
        opponent_car_idx:         u8,
        car_race_position:        u8,
        closing_rate_sec_per_lap: f32,
        slope_info:               SlopeInfo,
        prior_skirmishes:         u32,
        prior_attack_time_s:      f32,
    },
    HorizonClosing {
        lap:                       u8,
        session_time:              f32,
        attacker_car_idx:          u8,
        defender_car_idx:          u8,
        attacker_position:         u8,
        defender_position:         u8,
        current_gap_s:             f32,
        closing_rate_sec_per_lap:  f32,
        estimated_laps_to_contact: u16,
    },
    HorizonClosingResolved {
        lap:              u8,
        session_time:     f32,
        attacker_car_idx: u8,
        defender_car_idx: u8,
    },
    RaceGreen {
        lap:          u8,
        session_time: f32,
    },
    FlagYellowFullCourse {
        lap:          u8,
        session_time: f32,
    },
    FlagYellowLocal {
        lap:                u8,
        session_time:       f32,
        /// Car index of the vehicle that caused the yellow, if determinable.
        trigger_car_idx:    Option<u8>,
        /// Player's track position as a fraction (0.0–1.0) at the time of the yellow.
        lap_dist_pct:       Option<f32>,
        /// Track sector number, if available.
        sector:             Option<u8>,
        /// Classification of the yellow relative to the player.
        scope:              FlagScope,
        /// Cross-reference to an `IncidentCluster` bucket, if a recent incident
        /// was the likely cause of this yellow.
        linked_incident_id: Option<u32>,
    },
    RaceCheckered {
        lap:          u8,
        session_time: f32,
    },
    Overtake {
        lap:                u8,
        session_time:       f32,
        car_idx:            u8,
        overtaken_car_idx:  Option<u8>,
        position_from:      u8,
        position_to:        u8,
        positions_gained:   u8,
    },
    OvertakeForLead {
        lap:               u8,
        session_time:      f32,
        car_idx:           u8,
        overtaken_car_idx: Option<u8>,
        position_from:     u8,
        positions_gained:  u8,
    },
    LapCompleted {
        lap:             u8,
        session_time:    f32,
        player_car_idx:  u8,
        lap_time_s:      Option<f32>,
        best_lap_time_s: Option<f32>,
        position:        u8,
        pit_frames:      u32,
    },
    PitEntry {
        lap:            u8,
        session_time:   f32,
        player_car_idx: u8,
        position:       u8,
    },
    PitExit {
        lap:            u8,
        session_time:   f32,
        player_car_idx: u8,
        position:       u8,
    },
    TireDegradation {
        lap:                  u8,
        session_time:         f32,
        lf_temp_c:            f32,
        rf_temp_c:            f32,
        lr_temp_c:            f32,
        rr_temp_c:            f32,
        lf_slope_c_per_min:   f32,
        rf_slope_c_per_min:   f32,
        lr_slope_c_per_min:   f32,
        rr_slope_c_per_min:   f32,
        hottest_corner:       String,
    },
    FuelProjection {
        lap:              u8,
        session_time:     f32,
        fuel_remaining_l: f32,
        fuel_per_lap_l:   f32,
        laps_remaining:   f32,
        is_provisional:   bool,
    },
    FuelSavingTechnique {
        lap:                      u8,
        session_time:             f32,
        coast_duration_s:         f32,
        coast_start_lap_dist_pct: f32,
        coast_start_speed_mps:    f32,
    },
    MicroSectorGain {
        lap:               u8,
        session_time:      f32,
        bucket_from:       u8,
        bucket_to:         u8,
        lap_dist_pct_from: f32,
        lap_dist_pct_to:   f32,
        cumulative_delta_s: f32,
        technique_hint:    String,
    },
    MicroSectorLoss {
        lap:               u8,
        session_time:      f32,
        bucket_from:       u8,
        bucket_to:         u8,
        lap_dist_pct_from: f32,
        lap_dist_pct_to:   f32,
        cumulative_delta_s: f32,
    },
    BrakingProfile {
        lap:                  u8,
        session_time:         f32,
        anchor_bucket:        u8,
        brake_point_pct:      f32,
        brake_release_pct:    f32,
        peak_brake_pct:       f32,
        braking_energy:       f32,
        entry_speed_mps:      f32,
        min_speed_mps:        f32,
        throttle_release_pct: f32,
    },
    TrafficIntercept {
        lap:                            u8,
        session_time:                   f32,
        leader_car_idx:                 u8,
        traffic_car_idx:                u8,
        cross_class:                    bool,
        distance_m:                     f32,
        relative_speed_mps:             f32,
        time_to_intercept_s:            f32,
        intercept_bucket:               u8,
        intercept_lap_dist_pct:         f32,
        /// Absolute iRacing SessionTime (seconds) at which the intercept is
        /// predicted to occur: `session_time + time_to_intercept_s`.
        /// Use this instead of `time_to_intercept_s` to compute remaining
        /// lead time at read time — it is invariant to pipeline latency.
        predicted_intercept_session_time: f32,
    },
    VulnerabilityAlert {
        lap:                    u8,
        session_time:           f32,
        vulnerability:          f32,
        defender_idx:           u8,
        attacker_idx:           u8,
        tire_contribution:      f32,
        closing_contribution:   f32,
        proximity_contribution: f32,
        fuel_contribution:      f32,
    },
    VulnerabilityResolved {
        lap:          u8,
        session_time: f32,
        defender_idx: u8,
        attacker_idx: u8,
    },
    IncidentCluster {
        lap:               u8,
        session_time:      f32,
        bucket:            u8,
        lap_dist_pct_from: f32,
        lap_dist_pct_to:   f32,
        car_idxs:          Vec<u8>,
        severity:          f32,
        /// Most-culpable / dominant car in the incident (lowest car index when
        /// damage data is unavailable).
        primary_car_idx:   Option<u8>,
        /// Coarse classification of the incident (e.g. `"Incident"`).
        incident_type:     Option<String>,
    },
    IncidentClusterResolved {
        lap:          u8,
        session_time: f32,
        bucket:       u8,
    },
    TrafficCompressionZone {
        lap:                    u8,
        session_time:           f32,
        battle_attacker_idx:    u8,
        battle_defender_idx:    u8,
        window_start_pct:       f32,
        window_end_pct:         f32,
        traffic_car_idxs:       Vec<u8>,
        compression_score:      u8,
        first_intercept_car_idx: u8,
        first_intercept_time_s: f32,
        first_intercept_bucket: u8,
    },
    PublisherHello {
        lap:          u8,
        session_time: f32,
        version:      String,
        scope:        String,
    },
    PublisherGoodbye {
        lap:          u8,
        session_time: f32,
    },
}
