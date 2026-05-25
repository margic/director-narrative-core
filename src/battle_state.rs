/// Race-threat state for a single direction (forward or defensive).
/// Full implementation in issue #7.
#[derive(Debug, Clone, PartialEq)]
pub enum BattleState {
    Idle,
    Tracking,
    Push,
    AttackSetup,
}
