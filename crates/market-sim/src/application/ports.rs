use crate::domain::units::{AgentId, MarketStep};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ShockKind {
    Fundamental = 0,
    AgentSignal = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShockAddress {
    pub market_seed: u64,
    pub step: MarketStep,
    pub agent: Option<AgentId>,
    pub kind: ShockKind,
    pub draw_index: u32,
}

pub trait ShockSource {
    /// Returns a raw draw at a stable address. Identical addresses must replay.
    fn draw(&self, address: ShockAddress) -> u64;
}
