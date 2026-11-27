use super::hedging::TargetPosition;
use super::units::AgentId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AgentState {
    Fundamentalist {
        id: AgentId,
        signal: f64,
    },
    TrendTrader {
        id: AgentId,
        signal: f64,
    },
    NoiseTrader {
        id: AgentId,
        signal: f64,
    },
    Hedger {
        id: AgentId,
        target: Option<TargetPosition>,
    },
}

impl AgentState {
    pub const fn id(self) -> AgentId {
        match self {
            Self::Fundamentalist { id, .. }
            | Self::TrendTrader { id, .. }
            | Self::NoiseTrader { id, .. }
            | Self::Hedger { id, .. } => id,
        }
    }

    pub(crate) fn decide(
        &mut self,
        fundamental_gap: f64,
        last_log_return: f64,
        innovation: impl FnOnce() -> f64,
        hedge_target: impl FnOnce() -> TargetPosition,
    ) -> AgentDecision {
        let sensitivity = 1.0 + 0.05 * f64::from(self.id().get() % 11);
        let signal = match self {
            Self::Fundamentalist { signal, .. } => {
                *signal =
                    0.60 * *signal + 0.40 * sensitivity * fundamental_gap + 0.008 * innovation();
                *signal
            }
            Self::TrendTrader { signal, .. } => {
                *signal =
                    0.70 * *signal + 0.30 * sensitivity * last_log_return + 0.008 * innovation();
                *signal
            }
            Self::NoiseTrader { signal, .. } => {
                *signal = 0.60 * *signal + 0.012 * innovation();
                *signal
            }
            Self::Hedger { target, .. } => {
                let next_target = hedge_target();
                *target = Some(next_target);
                return AgentDecision::Hedge(next_target);
            }
        };
        AgentDecision::Background(signal)
    }
}

pub(crate) enum AgentDecision {
    Background(f64),
    Hedge(TargetPosition),
}

/// In ascending AgentId order, fundamentalists precede trend traders,
/// noise traders and hedgers.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentPopulation {
    agents: Vec<AgentState>,
    hedger_start: usize,
}

impl AgentPopulation {
    pub(crate) fn new(
        fundamentalists: u32,
        trend_traders: u32,
        noise_traders: u32,
        hedgers: u32,
    ) -> Self {
        let trend_start = fundamentalists;
        let noise_start = trend_start + trend_traders;
        let hedge_start = noise_start + noise_traders;
        let end = hedge_start + hedgers;
        let agents = (0..trend_start)
            .map(|id| AgentState::Fundamentalist {
                id: AgentId::new(id),
                signal: 0.0,
            })
            .chain(
                (trend_start..noise_start).map(|id| AgentState::TrendTrader {
                    id: AgentId::new(id),
                    signal: 0.0,
                }),
            )
            .chain(
                (noise_start..hedge_start).map(|id| AgentState::NoiseTrader {
                    id: AgentId::new(id),
                    signal: 0.0,
                }),
            )
            .chain((hedge_start..end).map(|id| AgentState::Hedger {
                id: AgentId::new(id),
                target: None,
            }))
            .collect();
        Self {
            agents,
            hedger_start: hedge_start as usize,
        }
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn agents(&self) -> &[AgentState] {
        &self.agents
    }

    pub fn hedgers(&self) -> impl ExactSizeIterator<Item = AgentId> + '_ {
        self.agents[self.hedger_start..]
            .iter()
            .map(|agent| agent.id())
    }

    pub(crate) fn agents_mut(&mut self) -> &mut [AgentState] {
        &mut self.agents
    }
}
