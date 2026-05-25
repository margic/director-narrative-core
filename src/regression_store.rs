/// Stores per-(bucket, car_idx) time series and computes OLS slopes on demand.
/// Full implementation in issue #6.
pub struct RegressionStore;

impl RegressionStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RegressionStore {
    fn default() -> Self {
        Self::new()
    }
}
