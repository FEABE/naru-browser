//! Ephemeral native start-page actions are bound to the selected document.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartPageTarget {
    pub tab: i32,
    pub space: String,
    pub navigation_epoch: u64,
}
