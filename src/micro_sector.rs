use crate::race_event::RaceEvent;

pub struct MicroSectorTracker {
    pub best_seg: Vec<Option<f32>>,
    pub current_lap_times: Vec<Option<f32>>,
    pub last_anchor_time: Option<f32>,
    pub last_anchor_bucket: Option<u8>,
    pub clean_laps_completed: u32,
}

impl MicroSectorTracker {
    pub fn new(anchor_count: usize) -> Self {
        Self {
            best_seg: vec![None; anchor_count],
            current_lap_times: vec![None; anchor_count],
            last_anchor_time: None,
            last_anchor_bucket: None,
            clean_laps_completed: 0,
        }
    }

    pub fn on_anchor_crossing(&mut self, session_time: f32, bucket: u8) {
        if let Some(last_time) = self.last_anchor_time {
            if let Some(last_bucket) = self.last_anchor_bucket {
                if bucket != last_bucket {
                    let segment_time = session_time - last_time;
                    if let Some(slot) = self.current_lap_times.get_mut(bucket as usize) {
                        *slot = Some(segment_time.max(0.0));
                    }
                }
            }
        }
        self.last_anchor_time = Some(session_time);
        self.last_anchor_bucket = Some(bucket);
    }

    pub fn on_lap_end(&mut self, lap: u8, session_time: f32, clean_lap: bool) -> Vec<RaceEvent> {
        let mut events = Vec::new();
        if clean_lap && self.clean_laps_completed > 0 {
            let mut streak_start = None;
            let mut streak_delta = 0.0;
            let mut streak_end = 0u8;
            for idx in 0..self.current_lap_times.len() {
                match (self.current_lap_times[idx], self.best_seg[idx]) {
                    (Some(cur), Some(best)) if cur < best => {
                        if streak_start.is_none() {
                            streak_start = Some(idx as u8);
                            streak_delta = 0.0;
                        }
                        streak_delta += best - cur;
                        streak_end = idx as u8;
                    }
                    _ => {
                        if let Some(start) = streak_start.take() {
                            events.push(gain_event(lap, session_time, start, streak_end, streak_delta, self.current_lap_times.len()));
                        }
                    }
                }
            }
            if let Some(start) = streak_start.take() {
                events.push(gain_event(lap, session_time, start, streak_end, streak_delta, self.current_lap_times.len()));
            }
        }

        if clean_lap {
            for (best, current) in self.best_seg.iter_mut().zip(self.current_lap_times.iter()) {
                if let Some(cur) = current {
                    match best {
                        Some(existing) if *existing <= *cur => {}
                        _ => *best = Some(*cur),
                    }
                }
            }
            self.clean_laps_completed += 1;
        }
        self.current_lap_times.fill(None);
        self.last_anchor_time = None;
        self.last_anchor_bucket = None;
        events
    }
}

fn gain_event(lap: u8, session_time: f32, start: u8, end: u8, delta: f32, anchor_count: usize) -> RaceEvent {
    RaceEvent::MicroSectorGain {
        lap,
        session_time,
        bucket_from: start,
        bucket_to: end,
        lap_dist_pct_from: start as f32 / anchor_count as f32,
        lap_dist_pct_to: (end as f32 + 1.0) / anchor_count as f32,
        cumulative_delta_s: delta,
        technique_hint: "carried more speed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_gain_for_improved_anchor_run() {
        let mut tracker = MicroSectorTracker::new(20);
        for bucket in 0..20u8 {
            tracker.on_anchor_crossing(bucket as f32, bucket);
        }
        tracker.on_lap_end(1, 60.0, true);

        for bucket in 0..20u8 {
            let t = if (12..=15).contains(&bucket) { bucket as f32 - 0.075 * (bucket - 11) as f32 } else { bucket as f32 };
            tracker.on_anchor_crossing(t, bucket);
        }
        let events = tracker.on_lap_end(2, 120.0, true);
        assert!(events.iter().any(|event| matches!(event, RaceEvent::MicroSectorGain { bucket_from: 12, bucket_to: 15, .. })));
    }
}
