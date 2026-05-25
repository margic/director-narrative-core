/// Captures the first gap reading per (lap, bucket, car_idx) spatial anchor.
/// Full implementation in issue #6.
pub struct AnchorSampler;

impl AnchorSampler {
    pub fn new(_n_buckets: usize) -> Self {
        Self
    }
}
