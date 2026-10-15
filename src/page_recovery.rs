//! Bounded native recovery state. Raw page URLs and error messages stay in the browser.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFailure {
    Load { code: i32 },
    Renderer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRecoveryTarget {
    pub tab: i32,
    pub navigation_epoch: u64,
    pub space: String,
    pub failure: PageFailure,
}
