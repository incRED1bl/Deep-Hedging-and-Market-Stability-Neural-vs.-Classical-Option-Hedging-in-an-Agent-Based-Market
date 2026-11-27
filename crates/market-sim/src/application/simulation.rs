use std::collections::BTreeMap;
use std::fmt;
use std::io;

use super::market_day::{CohortPositions, MarketDay, MarketDayError, MarketRecord, MarketState};
use super::ports::ShockSource;
use super::scenario::Scenario;
use crate::domain::accounting::ledger::{AccountingError, Ledger, Posting, Rotation};
use crate::domain::accounting::{Portfolio, PremiumPerShare};
use crate::domain::classical::call_terms;
use crate::domain::hedging::{
    CallExposure, CallObservation, HedgingPolicy, Moneyness, Observation, PolicyError,
    TargetPosition,
};
use crate::domain::units::{
    AgentId, CashUnits, ContractId, DomainError, Money, NetLots, PriceTicks, TradingDays,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalValuation {
    pub live_call_marks: BTreeMap<ContractId, PremiumPerShare>,
    pub lifetime_hedger_pnl: Vec<(AgentId, Money)>,
    pub observed_hedger_pnl: Vec<(AgentId, Money)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunMetadata {
    pub scenario: Scenario,
    pub fee_account: AgentId,
    pub funding_account: AgentId,
    pub call_buyer: AgentId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DayRecord {
    pub market: MarketRecord,
    pub included_in_metrics: bool,
    pub metadata: Option<RunMetadata>,
    pub postings: Vec<Posting>,
    pub burn_in_marks: Option<BTreeMap<ContractId, PremiumPerShare>>,
    pub terminal: Option<TerminalValuation>,
}

pub trait JournalSink {
    /// Publish one complete record and leave earlier records intact on error.
    fn publish(&mut self, day: &DayRecord) -> io::Result<()>;
}

#[derive(Debug)]
pub enum SimulationError {
    Domain(DomainError),
    Market(MarketDayError),
    Accounting(AccountingError),
    Policy(PolicyError),
    Journal(io::Error),
}

impl From<DomainError> for SimulationError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<MarketDayError> for SimulationError {
    fn from(error: MarketDayError) -> Self {
        Self::Market(error)
    }
}

impl From<AccountingError> for SimulationError {
    fn from(error: AccountingError) -> Self {
        Self::Accounting(error)
    }
}

impl fmt::Display for SimulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => error.fmt(formatter),
            Self::Market(error) => write!(formatter, "market day: {error:?}"),
            Self::Accounting(error) => error.fmt(formatter),
            Self::Policy(error) => write!(formatter, "hedging batch: {error:?}"),
            Self::Journal(error) => write!(formatter, "journal: {error}"),
        }
    }
}

impl std::error::Error for SimulationError {}

pub struct Simulation<'a> {
    scenario: Scenario,
    market: MarketDay,
    ledger: Ledger,
    rotation: Rotation,
    hedgers: Vec<AgentId>,
    pnl_baseline: Vec<Money>,
    last_step: u32,
    policy: &'a dyn HedgingPolicy,
    shocks: &'a dyn ShockSource,
    journal: &'a mut dyn JournalSink,
    positions: Vec<NetLots>,
    opening: Vec<Portfolio>,
    observations: Vec<Observation>,
    targets: Vec<TargetPosition>,
}

impl<'a> Simulation<'a> {
    pub fn new(
        scenario: Scenario,
        policy: &'a dyn HedgingPolicy,
        shocks: &'a dyn ShockSource,
        journal: &'a mut dyn JournalSink,
    ) -> Result<Self, SimulationError> {
        let market = MarketDay::new(&scenario)?;
        let count = market.population().len();
        let currency = scenario.instrument.tick_value_per_share.get().currency();
        let cash = Money::new(
            currency,
            CashUnits::new(100_000 * scenario.cash_decimal_places.units_per_currency()),
        );
        let opening_portfolio = Portfolio {
            net_lots: NetLots::new(0),
            cash,
        };
        let mut opening = vec![opening_portfolio; count + 3];
        for portfolio in &mut opening[count..count + 2] {
            portfolio.cash = Money::new(currency, CashUnits::new(0));
        }

        let fee_account = AgentId::new(count as u32);
        let funding_account = AgentId::new(count as u32 + 1);
        let call_buyer = AgentId::new(count as u32 + 2);
        let hedgers: Vec<_> = market.population().hedgers().collect();
        let pairs = hedgers.iter().map(|&seller| (call_buyer, seller)).collect();
        let rotation = Rotation::new(pairs, scenario.call.size, scenario.call.maturity)?;
        let mut ledger = Ledger::new(scenario.instrument, &opening, fee_account, funding_account)?;
        let state = market.state();
        rotation.issue_due_with(&mut ledger, state.step, state.close, || {
            quote_call(scenario, state, state.close, scenario.call.maturity)
                .map_err(SimulationError::from)
        })?;
        journal
            .publish(&DayRecord {
                market: MarketRecord {
                    state,
                    fills: Vec::new(),
                    traded_lots: 0,
                    positions: CohortPositions::default(),
                },
                included_in_metrics: false,
                metadata: Some(RunMetadata {
                    scenario,
                    fee_account,
                    funding_account,
                    call_buyer,
                }),
                postings: ledger.postings().to_vec(),
                burn_in_marks: None,
                terminal: None,
            })
            .map_err(SimulationError::Journal)?;

        let last_step = scenario.burn_in.get() + scenario.observed_days.get();
        Ok(Self {
            scenario,
            market,
            ledger,
            rotation,
            last_step,
            policy,
            shocks,
            journal,
            positions: vec![NetLots::new(0); count],
            opening,
            observations: Vec::with_capacity(hedgers.len()),
            targets: Vec::with_capacity(hedgers.len()),
            pnl_baseline: vec![Money::new(currency, CashUnits::new(0)); hedgers.len()],
            hedgers,
        })
    }

    pub fn state(&self) -> MarketState {
        self.market.state()
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn run(&mut self) -> Result<(), SimulationError> {
        while self.advance()? {}
        Ok(())
    }

    /// Return false only when every configured trading day has been published.
    pub fn advance(&mut self) -> Result<bool, SimulationError> {
        let state = self.state();
        if state.step.get() == self.last_step {
            return Ok(false);
        }

        self.observations.clear();
        self.targets.clear();
        self.opening.copy_from_slice(self.ledger.portfolios());
        for &id in &self.hedgers {
            let portfolio = self.opening[id.get() as usize];
            let call = self.ledger.active_call(id).map(|contract| CallObservation {
                moneyness: Moneyness::new(
                    state.close.get() as f64 / contract.strike().get() as f64,
                )
                .expect("moneyness is a ratio of positive price ticks"),
                time_to_expiry: TradingDays::new(contract.expires_at().get() - state.step.get()),
                underlying_exposure: CallExposure::Short(contract.size()),
            });
            let shares_held = self.scenario.instrument.shares(portfolio.net_lots);
            self.observations.push(Observation {
                as_of: state.step,
                shares_held,
                annualized_volatility: state.annualized_volatility,
                commission: self.scenario.commission,
                call,
            });
            self.targets.push(TargetPosition {
                target_shares: shares_held,
            });
        }
        self.policy
            .targets(&self.observations, &mut self.targets)
            .map_err(SimulationError::Policy)?;
        collect_positions(&self.opening, &mut self.positions);
        let prepared = self
            .market
            .prepare(self.shocks, &self.positions, &self.targets)?;
        let next = prepared.state();
        let scenario = self.scenario;
        let last = next.step.get() == self.last_step;
        let (new_market, baseline) =
            self.ledger
                .transact(|ledger| -> Result<_, SimulationError> {
                    let posting_start = ledger.postings().len();
                    for &fill in prepared.fills() {
                        ledger.book_fill(next.step, fill, scenario.commission)?;
                    }
                    ledger.accrue_funding(
                        next.step,
                        &self.opening,
                        scenario.funding_rate,
                        scenario.trading_days_per_year,
                    )?;
                    ledger.settle_expiring(next.step, next.close)?;
                    let value_at_close = |ledger| {
                        value_hedgers(scenario, next, ledger, &self.hedgers, &self.pnl_baseline)
                    };
                    let terminal = if last {
                        Some(value_at_close(ledger)?)
                    } else {
                        self.rotation
                            .issue_due_with(ledger, next.step, next.close, || {
                                quote_call(scenario, next, next.close, scenario.call.maturity)
                                    .map_err(SimulationError::from)
                            })?;
                        None
                    };
                    let (baseline, burn_in_marks) = if next.step.get() == scenario.burn_in.get() {
                        let cutoff = value_at_close(ledger)?;
                        (
                            Some(
                                cutoff
                                    .lifetime_hedger_pnl
                                    .into_iter()
                                    .map(|(_, pnl)| pnl)
                                    .collect(),
                            ),
                            Some(cutoff.live_call_marks),
                        )
                    } else {
                        (None, None)
                    };
                    ledger.check_conservation()?;
                    collect_positions(ledger.portfolios(), &mut self.positions);
                    let (market, record) = prepared.finish(&self.positions)?;
                    self.journal
                        .publish(&DayRecord {
                            market: record,
                            included_in_metrics: next.step.get() > scenario.burn_in.get(),
                            metadata: None,
                            postings: ledger.postings()[posting_start..].to_vec(),
                            burn_in_marks,
                            terminal,
                        })
                        .map_err(SimulationError::Journal)?;
                    Ok((market, baseline))
                })?;
        if let Some(baseline) = baseline {
            self.pnl_baseline = baseline;
        }
        self.market = new_market;
        Ok(true)
    }
}

fn collect_positions(portfolios: &[Portfolio], positions: &mut [NetLots]) {
    for (lots, portfolio) in positions.iter_mut().zip(portfolios) {
        *lots = portfolio.net_lots;
    }
}

fn quote_call(
    scenario: Scenario,
    state: MarketState,
    strike: PriceTicks,
    remaining: TradingDays,
) -> Result<PremiumPerShare, DomainError> {
    let years = f64::from(remaining.get()) / f64::from(scenario.trading_days_per_year.get());
    let strike_ticks = strike.get() as f64;
    let (_, price_ratio) = call_terms(
        state.close.get() as f64 / strike_ticks,
        years,
        scenario.funding_rate.get(),
        state.annualized_volatility.get(),
    );
    let tick_cash = scenario.instrument.tick_value_per_share.get();
    let units = (price_ratio * strike_ticks * tick_cash.amount().get() as f64).round();
    if !((0.0)..(-(i64::MIN as f64))).contains(&units) {
        return Err(DomainError::Overflow);
    }
    PremiumPerShare::new(Money::new(
        tick_cash.currency(),
        CashUnits::new(units as i64),
    ))
}

fn value_hedgers(
    scenario: Scenario,
    state: MarketState,
    ledger: &Ledger,
    hedgers: &[AgentId],
    baseline: &[Money],
) -> Result<TerminalValuation, SimulationError> {
    let mut live_call_marks = BTreeMap::new();
    for contract in ledger.active_calls() {
        let remaining = TradingDays::new(contract.expires_at().get() - state.step.get());
        let mark = quote_call(scenario, state, contract.strike(), remaining)?;
        live_call_marks.insert(contract.id(), mark);
    }
    let mut lifetime_hedger_pnl = Vec::with_capacity(hedgers.len());
    let mut observed_hedger_pnl = Vec::with_capacity(hedgers.len());
    for (&agent, &cutoff) in hedgers.iter().zip(baseline) {
        let pnl = ledger.pnl(agent, scenario.initial_close, state.close, &live_call_marks)?;
        lifetime_hedger_pnl.push((agent, pnl));
        observed_hedger_pnl.push((agent, pnl.checked_sub(cutoff)?));
    }
    Ok(TerminalValuation {
        live_call_marks,
        lifetime_hedger_pnl,
        observed_hedger_pnl,
    })
}
