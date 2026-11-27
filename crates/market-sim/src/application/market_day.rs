use super::ports::{ShockAddress, ShockKind, ShockSource};
use super::scenario::Scenario;
use crate::domain::agents::{AgentDecision, AgentPopulation, AgentState};
use crate::domain::hedging::TargetPosition;
use crate::domain::market::auction::{AuctionError, AuctionResult, clear_auction};
use crate::domain::market::{Fill, Order, Side};
use crate::domain::units::{
    AgentId, AnnualizedVolatility, DomainError, GrossReturn, MarketStep, NetLots, PriceTicks,
};

const VARIANCE_DECAY: f64 = 0.94;
const FULL_ORDER_SIGNAL: f64 = 0.03;
const MAX_BACKGROUND_LIMIT_MOVE: f64 = 0.05;
const HEDGE_LIMIT_MOVE: f64 = 0.02;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketDayError {
    Domain(DomainError),
    Auction(AuctionError),
    PositionCount {
        expected: usize,
        actual: usize,
    },
    ClosingPositionMismatch {
        agent: AgentId,
        expected: NetLots,
        actual: NetLots,
    },
    HedgeTargetCount {
        expected: usize,
        actual: usize,
    },
}

impl From<DomainError> for MarketDayError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<AuctionError> for MarketDayError {
    fn from(error: AuctionError) -> Self {
        Self::Auction(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarketState {
    pub step: MarketStep,
    pub close: PriceTicks,
    pub gross_return: GrossReturn,
    pub annualized_volatility: AnnualizedVolatility,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PositionAggregate {
    pub net_lots: i128,
    pub gross_lots: u128,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CohortPositions {
    pub fundamentalists: PositionAggregate,
    pub trend_traders: PositionAggregate,
    pub noise_traders: PositionAggregate,
    pub hedgers: PositionAggregate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketRecord {
    pub state: MarketState,
    pub fills: Vec<Fill>,
    pub traded_lots: u64,
    pub positions: CohortPositions,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketDay {
    scenario: Scenario,
    state: MarketState,
    log_fundamental: f64,
    population: AgentPopulation,
}

impl MarketDay {
    pub fn new(scenario: &Scenario) -> Result<Self, MarketDayError> {
        scenario.validate()?;
        let counts = scenario.population;
        Ok(Self {
            scenario: *scenario,
            state: MarketState {
                step: MarketStep::new(0),
                close: scenario.initial_close,
                gross_return: GrossReturn::new(1.0).expect("unit gross return is valid"),
                annualized_volatility: scenario.initial_volatility,
            },
            log_fundamental: (scenario.initial_close.get() as f64).ln(),
            population: AgentPopulation::new(
                counts.fundamentalists,
                counts.trend_traders,
                counts.noise_traders,
                counts.hedgers,
            ),
        })
    }

    pub const fn state(&self) -> MarketState {
        self.state
    }

    pub const fn population(&self) -> &AgentPopulation {
        &self.population
    }

    /// Uses the last committed close and positions to prepare the next auction.
    /// Holdings are in ascending AgentId order. Targets are in hedger order.
    pub fn prepare(
        &self,
        shocks: &(impl ShockSource + ?Sized),
        opening_positions: &[NetLots],
        hedge_targets: &[TargetPosition],
    ) -> Result<PreparedDay, MarketDayError> {
        self.check_position_count(opening_positions)?;
        let hedger_count = self.population.hedgers().len();
        if hedge_targets.len() != hedger_count {
            return Err(MarketDayError::HedgeTargetCount {
                expected: hedger_count,
                actual: hedge_targets.len(),
            });
        }

        let MarketState {
            step,
            close,
            gross_return,
            annualized_volatility,
        } = self.state;
        let next_step = MarketStep::new(step.get().checked_add(1).ok_or(DomainError::Overflow)?);
        let mut next = self.clone();
        let anchor = (self.scenario.initial_close.get() as f64).ln();
        let public_shock = self.shock(shocks, None, ShockKind::Fundamental);
        next.log_fundamental += 0.01 * (anchor - self.log_fundamental) + 0.015 * public_shock;
        let fundamental_gap = next.log_fundamental - (close.get() as f64).ln();
        let last_log_return = gross_return.get().ln();

        let mut targets = hedge_targets.iter().copied();
        let mut orders = Vec::with_capacity(self.population.len());
        for (agent, &held) in next
            .population
            .agents_mut()
            .iter_mut()
            .zip(opening_positions)
        {
            let id = agent.id();
            let decision = agent.decide(
                fundamental_gap,
                last_log_return,
                || self.shock(shocks, Some(id), ShockKind::AgentSignal),
                || targets.next().expect("one target per hedger"),
            );
            if let Some(order) = self.order(id, held, decision)? {
                orders.push(order);
            }
        }
        let auction = clear_auction(next_step, close, &orders)?;

        let ratio = auction.close.get() as f64 / close.get() as f64;
        let gross_return = GrossReturn::new(ratio).expect("ratio of positive ticks is positive");
        let annualized_return =
            ratio.ln() * f64::from(self.scenario.trading_days_per_year.get()).sqrt();
        let volatility = (VARIANCE_DECAY.sqrt() * annualized_volatility.get())
            .hypot((1.0 - VARIANCE_DECAY).sqrt() * annualized_return);
        next.state = MarketState {
            step: next_step,
            close: auction.close,
            gross_return,
            annualized_volatility: AnnualizedVolatility::new(volatility)
                .expect("weighted norm of annualized values is finite"),
        };
        let expected_closing_positions =
            positions_after_execution(opening_positions, &auction.fills);
        Ok(PreparedDay {
            next,
            auction,
            expected_closing_positions,
        })
    }

    fn order(
        &self,
        id: AgentId,
        held: NetLots,
        decision: AgentDecision,
    ) -> Result<Option<Order>, DomainError> {
        let limits = self.scenario.limits;
        let close = self.state.close;
        match decision {
            AgentDecision::Background(signal) => {
                let fraction = (signal / FULL_ORDER_SIGNAL).clamp(-1.0, 1.0);
                let quantity = (fraction * limits.max_order_lots().get() as f64).round() as i128;
                let limit = relative_limit(
                    close,
                    signal.clamp(-MAX_BACKGROUND_LIMIT_MOVE, MAX_BACKGROUND_LIMIT_MOVE),
                );
                limits.limit_order(id, held, quantity, limit)
            }
            AgentDecision::Hedge(target) => {
                let order =
                    limits.hedge_order(self.scenario.instrument, id, held, target, close)?;
                Ok(order.map(|order| {
                    let log_move = match order.side() {
                        Side::Buy => HEDGE_LIMIT_MOVE,
                        Side::Sell => -HEDGE_LIMIT_MOVE,
                    };
                    Order::new(
                        id,
                        order.side(),
                        order.lots(),
                        relative_limit(close, log_move),
                    )
                }))
            }
        }
    }

    fn check_position_count(&self, positions: &[NetLots]) -> Result<(), MarketDayError> {
        let expected = self.population.len();
        if positions.len() != expected {
            return Err(MarketDayError::PositionCount {
                expected,
                actual: positions.len(),
            });
        }
        Ok(())
    }

    fn shock(
        &self,
        shocks: &(impl ShockSource + ?Sized),
        agent: Option<AgentId>,
        kind: ShockKind,
    ) -> f64 {
        let draw = shocks.draw(ShockAddress {
            market_seed: self.scenario.seed,
            step: self.state.step,
            agent,
            kind,
            draw_index: 0,
        });
        // The highest 53 bits give exact uniform steps on [-1, 1).
        (draw >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedDay {
    next: MarketDay,
    auction: AuctionResult,
    expected_closing_positions: Vec<NetLots>,
}

impl PreparedDay {
    pub const fn state(&self) -> MarketState {
        self.next.state()
    }

    pub fn fills(&self) -> &[Fill] {
        &self.auction.fills
    }

    /// Completes the day after reconciling closing positions in AgentId order.
    pub fn finish(
        self,
        closing_positions: &[NetLots],
    ) -> Result<(MarketDay, MarketRecord), MarketDayError> {
        self.next.check_position_count(closing_positions)?;
        let mut positions = CohortPositions::default();
        for ((agent, &actual), expected) in self
            .next
            .population
            .agents()
            .iter()
            .zip(closing_positions)
            .zip(self.expected_closing_positions)
        {
            if actual != expected {
                return Err(MarketDayError::ClosingPositionMismatch {
                    agent: agent.id(),
                    expected,
                    actual,
                });
            }
            let aggregate = match agent {
                AgentState::Fundamentalist { .. } => &mut positions.fundamentalists,
                AgentState::TrendTrader { .. } => &mut positions.trend_traders,
                AgentState::NoiseTrader { .. } => &mut positions.noise_traders,
                AgentState::Hedger { .. } => &mut positions.hedgers,
            };
            aggregate.net_lots += i128::from(expected.get());
            aggregate.gross_lots += u128::from(expected.get().unsigned_abs());
        }
        let record = MarketRecord {
            state: self.next.state,
            fills: self.auction.fills,
            traded_lots: self.auction.traded_lots,
            positions,
        };
        Ok((self.next, record))
    }
}

fn positions_after_execution(opening_positions: &[NetLots], fills: &[Fill]) -> Vec<NetLots> {
    let mut positions = opening_positions.to_vec();
    for fill in fills {
        let quantity = i128::from(fill.lots().get());
        for (agent, change) in [(fill.buyer(), quantity), (fill.seller(), -quantity)] {
            let position = &mut positions[agent.get() as usize];
            let updated = i128::from(position.get()) + change;
            // One order per agent means each successive fill moves toward its risk-limited target.
            *position = NetLots::new(i64::try_from(updated).expect("fills honor position limits"));
        }
    }
    positions
}

fn relative_limit(close: PriceTicks, log_move: f64) -> PriceTicks {
    let price = (close.get() as f64 * log_move.exp()).round();
    PriceTicks::new((price as u64).max(1)).expect("rounded limit has a one-tick floor")
}
