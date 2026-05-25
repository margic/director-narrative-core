/// A single decoded telemetry frame from the iRacing session stream.
/// Serde `Deserialize` impl added in issue #9.
pub struct TelemetryFrame {
    pub lap:                  u8,
    pub session_time:         f32,
    pub lap_dist_pct:         f32,
    pub player_car_idx:       u8,
    pub player_car_position:  u8,
    pub on_pit_road:          bool,
    pub session_flags:        u32,
    /// Indexed by car_idx. Values < -0.5 are iRacing sentinels (inactive slot).
    pub car_idx_lap_dist_pct: Vec<f32>,
    /// Indexed by car_idx. 0 = inactive.
    pub car_idx_position:     Vec<u8>,
    /// Indexed by car_idx.
    pub car_idx_on_pit_road:  Vec<bool>,
}
