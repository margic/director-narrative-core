use serde::Serialize;
use strum::{Display, EnumIter, IntoEnumIterator};

use crate::battle_state::SlopeInfo;

/// High-level scope of an emitted event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventScope {
    CarScoped,
    RigScoped,
    SessionScoped,
}

/// Classification of a yellow flag's scope relative to the player.
#[derive(Debug, Serialize)]
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
    IracingConnected {
        lap:          u8,
        session_time: f32,
    },
    IracingDisconnected {
        lap:          u8,
        session_time: f32,
    },
    DriverEnteredCar {
        lap:            u8,
        session_time:   f32,
        player_car_idx: u8,
    },
    DriverExitedCar {
        lap:            u8,
        session_time:   f32,
        player_car_idx: u8,
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
    IncidentAlert {
        lap:                    u8,
        session_time:           f32,
        car_idx:                u8,
        driver_incident_count:  Option<i32>,
        previous_track_surface: i32,
        current_track_surface:  i32,
        previous_speed_mps:     f32,
        current_speed_mps:      f32,
        speed_drop_mps:         f32,
        /// Raw, unnormalised magnitude: the speed loss as a fraction of the
        /// prior speed for surface/speed signatures, or the iRacing incident
        /// count delta (1, 2, 4, …) for `incident_count_increase`.
        severity:               f32,
        /// Same magnitude mapped onto 0.0–1.0 regardless of `reason`, so a
        /// quality floor can be applied without knowing the signature.
        severity_normalized:    f32,
        /// iRacing incident points gained, for `incident_count_increase` only.
        incident_count_delta:   Option<i32>,
        reason:                 String,
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
    PublisherHeartbeat {
        lap:                   u8,
        session_time:          f32,
        version:               String,
        events_enqueued_total: u64,
    },
    /// Driver pressed their bound "focus on me" wheel button. Car-scoped so
    /// the sandbox can resolve the requesting car from the roster.
    FocusMeRequested {
        lap:             u8,
        session_time:    f32,
        player_car_idx:  u8,
        /// Idempotency key — stable across transport retries.
        request_id:      String,
        /// Monotonic per-publisher counter, orders presses inside one tick.
        press_seq:       u64,
        /// Stable driver identity: `user:<iRacing userId>` or `name:<normalized>`.
        driver_id:       String,
        rig_id:          String,
        /// `wheel_button` or `simulated`.
        source:          String,
        button:          u16,
        /// Wall-clock milliseconds since the Unix epoch.
        requested_at_ms: i64,
        /// Airtime the requesting driver asked for, in milliseconds.
        dwell_ms:        u32,
    },
    /// Periodic state of the driver at the wheel of this rig, emitted on a
    /// wall-clock cadence regardless of whether anything notable happened.
    ///
    /// The consumer needs a steady supply of material for the drivers it is
    /// meant to cover: a rig that produces no narrative events for minutes at a
    /// time otherwise looks stale next to a busy midfield battle.
    DriverMaterial {
        lap:               u8,
        session_time:      f32,
        player_car_idx:    u8,
        position:          u8,
        laps_completed:    i32,
        lap_dist_pct:      f32,
        last_lap_time_s:   Option<f32>,
        best_lap_time_s:   Option<f32>,
        /// Gap to the car ahead in race order, seconds. `None` when nobody is
        /// within a credible battle gap.
        gap_ahead_s:       Option<f32>,
        car_ahead_idx:     Option<u8>,
        gap_behind_s:      Option<f32>,
        car_behind_idx:    Option<u8>,
        on_pit_road:       bool,
        track_surface:     i32,
        speed_mps:         f32,
        fuel_level_l:      f32,
        incident_count:    i32,
        session_state:     i32,
        /// Wall-clock seconds since the previous `DRIVER_MATERIAL` from this rig.
        interval_s:        f32,
    },
    /// The publisher discarded all session-scoped state because iRacing moved to
    /// a different session. Consumers should drop cached car indices, battles,
    /// and driver bindings for `previous_sub_session_id` when they see this.
    SessionReset {
        lap:                     u8,
        session_time:            f32,
        previous_sub_session_id: Option<i64>,
        sub_session_id:          i64,
        previous_session_num:    Option<i32>,
        session_num:             i32,
        /// Machine-readable cause, e.g. `sub_session_changed`.
        reason:                  String,
    },
    /// Driver pressed their bound pause/resume button. Rig-scoped: it acts on
    /// the broadcast agent, not on a car.
    BroadcastControlRequested {
        lap:             u8,
        session_time:    f32,
        /// Always `toggle` — the sandbox owns pause/resume state.
        action:          String,
        request_id:      String,
        press_seq:       u64,
        driver_id:       String,
        rig_id:          String,
        source:          String,
        button:          u16,
        requested_at_ms: i64,
    },
}

/// Discriminant of [`RaceEvent`]: one variant per emitted event type, with the
/// payload fields stripped.
///
/// This exists so the wire contract can be enumerated without constructing an
/// event of every variant. [`RaceEvent::kind`] and [`RaceEventKind::scope`] are
/// exhaustive matches, so a new `RaceEvent` variant does not compile until it is
/// registered here and given a scope, and the catalog contract test then fails
/// until `contracts/publisher-event-catalog.json` is regenerated — which is the
/// signal that Race Control's ingest catalog needs the new type too.
///
/// `serialize_all` mirrors the `rename_all` on `RaceEvent`, so
/// [`RaceEventKind::event_type`] is the same string serde writes to the
/// envelope's `type` field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Display, EnumIter)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum RaceEventKind {
    BattleEngaged,
    BattleBroken,
    BattleClosing,
    HorizonClosing,
    HorizonClosingResolved,
    RaceGreen,
    FlagYellowFullCourse,
    FlagYellowLocal,
    RaceCheckered,
    IracingConnected,
    IracingDisconnected,
    DriverEnteredCar,
    DriverExitedCar,
    Overtake,
    OvertakeForLead,
    LapCompleted,
    PitEntry,
    PitExit,
    TireDegradation,
    FuelProjection,
    FuelSavingTechnique,
    IncidentAlert,
    MicroSectorGain,
    MicroSectorLoss,
    BrakingProfile,
    TrafficIntercept,
    VulnerabilityAlert,
    VulnerabilityResolved,
    IncidentCluster,
    IncidentClusterResolved,
    TrafficCompressionZone,
    PublisherHello,
    PublisherGoodbye,
    PublisherHeartbeat,
    FocusMeRequested,
    DriverMaterial,
    SessionReset,
    BroadcastControlRequested,
}

impl RaceEventKind {
    /// Every event type the publisher can emit, in declaration order.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::iter()
    }

    /// Wire value of the envelope's `type` field, e.g. `"BATTLE_ENGAGED"`.
    pub fn event_type(self) -> String {
        self.to_string()
    }

    pub fn scope(self) -> EventScope {
        match self {
            Self::RaceGreen
            | Self::RaceCheckered
            | Self::FlagYellowFullCourse
            | Self::SessionReset => EventScope::SessionScoped,
            Self::PublisherHello
            | Self::PublisherGoodbye
            | Self::PublisherHeartbeat
            | Self::IracingConnected
            | Self::IracingDisconnected
            | Self::BroadcastControlRequested => EventScope::RigScoped,
            Self::BattleEngaged
            | Self::BattleBroken
            | Self::BattleClosing
            | Self::HorizonClosing
            | Self::HorizonClosingResolved
            | Self::FlagYellowLocal
            | Self::DriverEnteredCar
            | Self::DriverExitedCar
            | Self::Overtake
            | Self::OvertakeForLead
            | Self::LapCompleted
            | Self::PitEntry
            | Self::PitExit
            | Self::TireDegradation
            | Self::FuelProjection
            | Self::FuelSavingTechnique
            | Self::IncidentAlert
            | Self::MicroSectorGain
            | Self::MicroSectorLoss
            | Self::BrakingProfile
            | Self::TrafficIntercept
            | Self::VulnerabilityAlert
            | Self::VulnerabilityResolved
            | Self::IncidentCluster
            | Self::IncidentClusterResolved
            | Self::TrafficCompressionZone
            | Self::DriverMaterial
            | Self::FocusMeRequested => EventScope::CarScoped,
        }
    }
}

impl RaceEvent {
    pub fn kind(&self) -> RaceEventKind {
        match self {
            Self::BattleEngaged { .. } => RaceEventKind::BattleEngaged,
            Self::BattleBroken { .. } => RaceEventKind::BattleBroken,
            Self::BattleClosing { .. } => RaceEventKind::BattleClosing,
            Self::HorizonClosing { .. } => RaceEventKind::HorizonClosing,
            Self::HorizonClosingResolved { .. } => RaceEventKind::HorizonClosingResolved,
            Self::RaceGreen { .. } => RaceEventKind::RaceGreen,
            Self::FlagYellowFullCourse { .. } => RaceEventKind::FlagYellowFullCourse,
            Self::FlagYellowLocal { .. } => RaceEventKind::FlagYellowLocal,
            Self::RaceCheckered { .. } => RaceEventKind::RaceCheckered,
            Self::IracingConnected { .. } => RaceEventKind::IracingConnected,
            Self::IracingDisconnected { .. } => RaceEventKind::IracingDisconnected,
            Self::DriverEnteredCar { .. } => RaceEventKind::DriverEnteredCar,
            Self::DriverExitedCar { .. } => RaceEventKind::DriverExitedCar,
            Self::Overtake { .. } => RaceEventKind::Overtake,
            Self::OvertakeForLead { .. } => RaceEventKind::OvertakeForLead,
            Self::LapCompleted { .. } => RaceEventKind::LapCompleted,
            Self::PitEntry { .. } => RaceEventKind::PitEntry,
            Self::PitExit { .. } => RaceEventKind::PitExit,
            Self::TireDegradation { .. } => RaceEventKind::TireDegradation,
            Self::FuelProjection { .. } => RaceEventKind::FuelProjection,
            Self::FuelSavingTechnique { .. } => RaceEventKind::FuelSavingTechnique,
            Self::IncidentAlert { .. } => RaceEventKind::IncidentAlert,
            Self::MicroSectorGain { .. } => RaceEventKind::MicroSectorGain,
            Self::MicroSectorLoss { .. } => RaceEventKind::MicroSectorLoss,
            Self::BrakingProfile { .. } => RaceEventKind::BrakingProfile,
            Self::TrafficIntercept { .. } => RaceEventKind::TrafficIntercept,
            Self::VulnerabilityAlert { .. } => RaceEventKind::VulnerabilityAlert,
            Self::VulnerabilityResolved { .. } => RaceEventKind::VulnerabilityResolved,
            Self::IncidentCluster { .. } => RaceEventKind::IncidentCluster,
            Self::IncidentClusterResolved { .. } => RaceEventKind::IncidentClusterResolved,
            Self::TrafficCompressionZone { .. } => RaceEventKind::TrafficCompressionZone,
            Self::PublisherHello { .. } => RaceEventKind::PublisherHello,
            Self::PublisherGoodbye { .. } => RaceEventKind::PublisherGoodbye,
            Self::PublisherHeartbeat { .. } => RaceEventKind::PublisherHeartbeat,
            Self::FocusMeRequested { .. } => RaceEventKind::FocusMeRequested,
            Self::DriverMaterial { .. } => RaceEventKind::DriverMaterial,
            Self::SessionReset { .. } => RaceEventKind::SessionReset,
            Self::BroadcastControlRequested { .. } => RaceEventKind::BroadcastControlRequested,
        }
    }

    pub fn event_scope(&self) -> EventScope {
        self.kind().scope()
    }
}
