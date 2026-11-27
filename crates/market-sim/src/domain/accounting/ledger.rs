//! Bilateral cash and position accounting for one instrument.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::{OptionContract, Portfolio, PremiumPerShare};
use crate::domain::{
    market::{Fill, InstrumentSpec},
    units::{
        AgentId, CashUnits, ContractId, CurrencyId, DomainError, FundingRate, LotSize, MarketStep,
        Money, NetLots, OneWayCommissionRate, PriceTicks, TradingDays,
    },
};

mod rotation;
pub use rotation::{CALL_TENOR, Rotation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Posting {
    pub step: MarketStep,
    pub kind: PostingKind,
}

/// A posting contains both sides of each transfer. Signed interest is the amount
/// the participant receives from the funding account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostingKind {
    Opening {
        agent: AgentId,
        portfolio: Portfolio,
    },
    Fill {
        fill: Fill,
        notional: Money,
        fee_per_side: Money,
        fee_account: AgentId,
    },
    Funding {
        agent: AgentId,
        funding_account: AgentId,
        interest: Money,
    },
    OptionIssue {
        contract: OptionContract,
        premium: Money,
    },
    OptionSettlement {
        contract: OptionContract,
        close: PriceTicks,
        payoff: Money,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountingError {
    Domain(DomainError),
    UnknownAgent(AgentId),
    ReservedAccount(AgentId),
    InvalidOpening,
    InvalidPosting,
    BackdatedPosting(MarketStep),
    DuplicateContract(ContractId),
    ActiveSeller(AgentId),
    OverdueContract(ContractId),
    InvalidSettlement(ContractId),
    InvalidFundingSnapshot,
    UnknownMark(ContractId),
    MissingMark(ContractId),
    Conservation,
}

impl From<DomainError> for AccountingError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl fmt::Display for AccountingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => error.fmt(formatter),
            Self::UnknownAgent(agent) => write!(formatter, "unknown agent {}", agent.get()),
            Self::ReservedAccount(agent) => write!(formatter, "reserved account {}", agent.get()),
            Self::InvalidOpening => formatter.write_str("invalid opening portfolios"),
            Self::InvalidPosting => {
                formatter.write_str("posting does not match the accounting record")
            }
            Self::BackdatedPosting(step) => {
                write!(formatter, "backdated market step {}", step.get())
            }
            Self::DuplicateContract(id) => {
                write!(formatter, "contract {} already issued", id.get())
            }
            Self::ActiveSeller(agent) => write!(
                formatter,
                "seller {} already has an active call",
                agent.get()
            ),
            Self::OverdueContract(id) => {
                write!(formatter, "contract {} missed its expiry", id.get())
            }
            Self::InvalidSettlement(id) => {
                write!(formatter, "invalid settlement for contract {}", id.get())
            }
            Self::InvalidFundingSnapshot => {
                formatter.write_str("invalid opening snapshot for funding")
            }
            Self::UnknownMark(id) => write!(formatter, "mark for inactive contract {}", id.get()),
            Self::MissingMark(id) => write!(formatter, "missing mark for contract {}", id.get()),
            Self::Conservation => formatter.write_str("cash or lots do not reconcile"),
        }
    }
}

impl std::error::Error for AccountingError {}

#[derive(Debug)]
pub struct Ledger {
    instrument: InstrumentSpec,
    opening: Vec<Portfolio>,
    portfolios: Vec<Portfolio>,
    fee_account: AgentId,
    funding_account: AgentId,
    issued: BTreeSet<ContractId>,
    active: BTreeMap<ContractId, OptionContract>,
    managed: BTreeMap<AgentId, ContractId>,
    last_funding_step: Option<MarketStep>,
    postings: Vec<Posting>,
}

impl Ledger {
    /// The opening slice is indexed by dense agent ID. Both reserve accounts must have zero lots.
    pub fn new(
        instrument: InstrumentSpec,
        opening: &[Portfolio],
        fee_account: AgentId,
        funding_account: AgentId,
    ) -> Result<Self, AccountingError> {
        let currency = instrument.tick_value_per_share.get().currency();
        if opening.len().saturating_sub(1) > u32::MAX as usize
            || fee_account == funding_account
            || [fee_account, funding_account].into_iter().any(|id| {
                opening
                    .get(agent_index(id))
                    .is_none_or(|portfolio| portfolio.net_lots.get() != 0)
            })
            || opening
                .iter()
                .any(|portfolio| portfolio.cash.currency() != currency)
        {
            return Err(AccountingError::InvalidOpening);
        }
        let postings = opening
            .iter()
            .copied()
            .enumerate()
            .map(|(index, portfolio)| Posting {
                step: MarketStep::new(0),
                kind: PostingKind::Opening {
                    agent: AgentId::new(index as u32),
                    portfolio,
                },
            })
            .collect();

        Ok(Self {
            instrument,
            opening: opening.to_vec(),
            portfolios: opening.to_vec(),
            fee_account,
            funding_account,
            issued: BTreeSet::new(),
            active: BTreeMap::new(),
            managed: BTreeMap::new(),
            last_funding_step: None,
            postings,
        })
    }

    /// Rebuild balances and active contracts from persisted opening and movement postings.
    pub fn replay(
        instrument: InstrumentSpec,
        fee_account: AgentId,
        funding_account: AgentId,
        postings: &[Posting],
    ) -> Result<Self, AccountingError> {
        let mut opening = Vec::new();
        for posting in postings {
            let PostingKind::Opening { agent, portfolio } = posting.kind else {
                break;
            };
            if posting.step != MarketStep::new(0) || agent_index(agent) != opening.len() {
                return Err(AccountingError::InvalidOpening);
            }
            opening.push(portfolio);
        }
        let mut ledger = Self::new(instrument, &opening, fee_account, funding_account)?;
        let mut funded_participants = BTreeSet::new();
        for posting in &postings[opening.len()..] {
            if let PostingKind::Fill { fill, notional, .. } = posting.kind
                && ledger.instrument.notional(fill.price(), fill.lots())? != notional
            {
                return Err(AccountingError::InvalidPosting);
            }
            if let PostingKind::Funding { agent, .. } = posting.kind {
                if ledger.last_funding_step != Some(posting.step) {
                    funded_participants.clear();
                    ledger.last_funding_step = Some(posting.step);
                }
                if !funded_participants.insert(agent) {
                    return Err(AccountingError::InvalidPosting);
                }
            }
            ledger.post(*posting)?;
        }
        Ok(ledger)
    }

    pub fn portfolios(&self) -> &[Portfolio] {
        &self.portfolios
    }

    pub fn portfolio(&self, agent: AgentId) -> Option<&Portfolio> {
        self.portfolios.get(agent_index(agent))
    }

    /// The single call managed by this seller. A buyer can own multiple calls.
    pub fn active_call(&self, seller: AgentId) -> Option<&OptionContract> {
        self.managed.get(&seller).and_then(|id| self.active.get(id))
    }

    pub fn active_calls(&self) -> impl Iterator<Item = &OptionContract> {
        self.active.values()
    }

    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }

    pub fn book_fill(
        &mut self,
        step: MarketStep,
        fill: Fill,
        commission: OneWayCommissionRate,
    ) -> Result<(), AccountingError> {
        let notional = self.instrument.notional(fill.price(), fill.lots())?;
        let fee_per_side = commission.apply(notional)?;
        self.post(Posting {
            step,
            kind: PostingKind::Fill {
                fill,
                notional,
                fee_per_side,
                fee_account: self.fee_account,
            },
        })
    }

    /// Apply one elapsed interval of interest to the cash captured before that day's fills.
    /// Retrying an already funded step is a no-op while that step is current.
    pub fn accrue_funding(
        &mut self,
        step: MarketStep,
        opening: &[Portfolio],
        rate: FundingRate,
        trading_days_per_year: TradingDays,
    ) -> Result<(), AccountingError> {
        if opening.len() != self.portfolios.len()
            || trading_days_per_year.get() == 0
            || opening
                .iter()
                .any(|portfolio| portfolio.cash.currency() != self.currency())
        {
            return Err(AccountingError::InvalidFundingSnapshot);
        }
        self.ensure_posting_step(step)?;
        if self.last_funding_step == Some(step) {
            return Ok(());
        }

        self.transact(|ledger| {
            let start = ledger.postings.len();
            let mut zero_posting = None;
            for (index, portfolio) in opening.iter().enumerate() {
                let agent = AgentId::new(index as u32);
                if agent == ledger.fee_account || agent == ledger.funding_account {
                    continue;
                }
                let interest = rate.daily_interest(portfolio.cash, trading_days_per_year)?;
                let posting = Posting {
                    step,
                    kind: PostingKind::Funding {
                        agent,
                        funding_account: ledger.funding_account,
                        interest,
                    },
                };
                if interest.amount().get() == 0 {
                    zero_posting.get_or_insert(posting);
                } else {
                    ledger.post(posting)?;
                }
            }
            if ledger.postings.len() == start {
                let Some(posting) = zero_posting else {
                    return Ok(());
                };
                // A zero-interest entry lets replay recover this completed funding day.
                ledger.post(posting)?;
            }
            ledger.last_funding_step = Some(step);
            Ok(())
        })
    }

    pub fn issue_call(&mut self, contract: OptionContract) -> Result<(), AccountingError> {
        let premium = contract.premium_per_share().total(contract.size())?;
        self.post(Posting {
            step: contract.issued_at(),
            kind: PostingKind::OptionIssue { contract, premium },
        })
    }

    /// Cash-settle contracts at the expiry close, including a zero-payoff record.
    pub fn settle_expiring(
        &mut self,
        step: MarketStep,
        close: PriceTicks,
    ) -> Result<usize, AccountingError> {
        let mut expiring = Vec::new();
        for contract in self.active_calls() {
            if contract.expires_at() < step {
                return Err(AccountingError::OverdueContract(contract.id()));
            }
            if contract.expires_at() == step {
                expiring.push(*contract);
            }
        }

        if expiring.is_empty() {
            return Ok(0);
        }

        self.transact(|ledger| {
            for &contract in &expiring {
                let payoff = ledger.payoff(contract, close)?;
                ledger.post(Posting {
                    step,
                    kind: PostingKind::OptionSettlement {
                        contract,
                        close,
                        payoff,
                    },
                })?;
            }
            Ok(expiring.len())
        })
    }

    /// Roll back a group of operations if the caller returns an error.
    /// The journal remains append-only on success.
    pub fn transact<T, E>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let portfolios = self.portfolios.clone();
        let active = self.active.clone();
        let managed = self.managed.clone();
        let last_funding_step = self.last_funding_step;
        let length = self.postings.len();
        let result = operation(self);
        if result.is_err() {
            self.portfolios = portfolios;
            self.active = active;
            self.managed = managed;
            self.last_funding_step = last_funding_step;
            for posting in self.postings.drain(length..) {
                if let PostingKind::OptionIssue { contract, .. } = posting.kind {
                    self.issued.remove(&contract.id());
                }
            }
        }
        result
    }

    /// Compare total cash and signed lots with the opening balances.
    pub fn check_conservation(&self) -> Result<(), AccountingError> {
        if totals(&self.opening) != totals(&self.portfolios) {
            return Err(AccountingError::Conservation);
        }
        Ok(())
    }

    /// Compute change in marked equity, including one quote-currency mark per live call.
    pub fn pnl(
        &self,
        agent: AgentId,
        opening_close: PriceTicks,
        terminal_close: PriceTicks,
        marks: &BTreeMap<ContractId, PremiumPerShare>,
    ) -> Result<Money, AccountingError> {
        let portfolio = *self
            .portfolio(agent)
            .ok_or(AccountingError::UnknownAgent(agent))?;
        let starting_portfolio = self.opening[agent_index(agent)];
        let mut change = self.amount(portfolio.cash)? - self.amount(starting_portfolio.cash)?;
        if portfolio.net_lots != starting_portfolio.net_lots || terminal_close != opening_close {
            change += self.stock_value(portfolio.net_lots, terminal_close)?
                - self.stock_value(starting_portfolio.net_lots, opening_close)?;
        }

        for (&id, mark) in marks {
            if !self.active.contains_key(&id) {
                return Err(AccountingError::UnknownMark(id));
            }
            self.amount(mark.get())?;
        }
        for contract in self.active_calls() {
            let mark = marks
                .get(&contract.id())
                .ok_or(AccountingError::MissingMark(contract.id()))?;
            let direction =
                i128::from(agent == contract.buyer()) - i128::from(agent == contract.seller());
            if direction != 0 {
                let value =
                    i128::from(mark.get().amount().get()) * i128::from(contract.size().get());
                change = change
                    .checked_add(direction * value)
                    .ok_or(DomainError::Overflow)?;
            }
        }
        self.money(change)
    }

    fn ensure_posting_step(&self, step: MarketStep) -> Result<(), AccountingError> {
        if self.postings.last().is_some_and(|last| step < last.step) {
            return Err(AccountingError::BackdatedPosting(step));
        }
        Ok(())
    }

    fn post(&mut self, posting: Posting) -> Result<(), AccountingError> {
        self.ensure_posting_step(posting.step)?;
        match posting.kind {
            PostingKind::Opening { .. } => return Err(AccountingError::InvalidOpening),
            PostingKind::Fill {
                fill,
                notional,
                fee_per_side,
                fee_account,
            } => {
                self.apply_fill(fill, notional, fee_per_side, fee_account)?;
            }
            PostingKind::Funding {
                agent,
                funding_account,
                interest,
            } => {
                if funding_account != self.funding_account {
                    return Err(AccountingError::InvalidPosting);
                }
                self.ensure_participant(agent)?;
                self.transfer_cash(funding_account, agent, self.amount(interest)?)?;
            }
            PostingKind::OptionIssue { contract, premium } => {
                self.apply_issue(posting.step, contract, premium)?;
            }
            PostingKind::OptionSettlement {
                contract,
                close,
                payoff,
            } => {
                self.apply_settlement(posting.step, contract, close, payoff)?;
            }
        }
        self.postings.push(posting);
        Ok(())
    }

    fn apply_fill(
        &mut self,
        fill: Fill,
        notional: Money,
        fee_per_side: Money,
        fee_account: AgentId,
    ) -> Result<(), AccountingError> {
        let notional = self.amount(notional)?;
        let fee = self.amount(fee_per_side)?;
        if fee_account != self.fee_account || notional < 0 || fee < 0 {
            return Err(AccountingError::InvalidPosting);
        }
        self.ensure_participant(fill.buyer())?;
        self.ensure_participant(fill.seller())?;
        let lots = i128::from(fill.lots().get());
        let buyer = self.adjusted(fill.buyer(), -notional - fee, lots)?;
        let seller = self.adjusted(fill.seller(), notional - fee, -lots)?;
        let fees = self.adjusted(fee_account, 2 * fee, 0)?;
        self.portfolios[agent_index(fill.buyer())] = buyer;
        self.portfolios[agent_index(fill.seller())] = seller;
        self.portfolios[agent_index(fee_account)] = fees;
        Ok(())
    }

    fn apply_issue(
        &mut self,
        step: MarketStep,
        contract: OptionContract,
        premium: Money,
    ) -> Result<(), AccountingError> {
        if step != contract.issued_at()
            || premium != contract.premium_per_share().total(contract.size())?
        {
            return Err(AccountingError::InvalidPosting);
        }
        self.ensure_participant(contract.buyer())?;
        self.ensure_participant(contract.seller())?;
        if self.issued.contains(&contract.id()) {
            return Err(AccountingError::DuplicateContract(contract.id()));
        }
        if self.managed.contains_key(&contract.seller()) {
            return Err(AccountingError::ActiveSeller(contract.seller()));
        }
        self.transfer_cash(contract.buyer(), contract.seller(), self.amount(premium)?)?;
        self.issued.insert(contract.id());
        self.active.insert(contract.id(), contract);
        self.managed.insert(contract.seller(), contract.id());
        Ok(())
    }

    fn apply_settlement(
        &mut self,
        step: MarketStep,
        contract: OptionContract,
        close: PriceTicks,
        payoff: Money,
    ) -> Result<(), AccountingError> {
        if self.active.get(&contract.id()) != Some(&contract)
            || contract.expires_at() != step
            || self.payoff(contract, close)? != payoff
        {
            return Err(AccountingError::InvalidSettlement(contract.id()));
        }
        self.transfer_cash(contract.seller(), contract.buyer(), self.amount(payoff)?)?;
        self.active.remove(&contract.id());
        self.managed.remove(&contract.seller());
        Ok(())
    }

    fn ensure_participant(&self, agent: AgentId) -> Result<(), AccountingError> {
        if agent == self.fee_account || agent == self.funding_account {
            return Err(AccountingError::ReservedAccount(agent));
        }
        Ok(())
    }

    fn transfer_cash(
        &mut self,
        from: AgentId,
        to: AgentId,
        amount: i128,
    ) -> Result<(), AccountingError> {
        let debit = self.adjusted(from, -amount, 0)?;
        let credit = self.adjusted(to, amount, 0)?;
        self.portfolios[agent_index(from)] = debit;
        self.portfolios[agent_index(to)] = credit;
        Ok(())
    }

    fn adjusted(
        &self,
        agent: AgentId,
        cash: i128,
        lots: i128,
    ) -> Result<Portfolio, AccountingError> {
        let portfolio = self
            .portfolio(agent)
            .ok_or(AccountingError::UnknownAgent(agent))?;
        let lots = i64::try_from(i128::from(portfolio.net_lots.get()) + lots)
            .map_err(|_| DomainError::Overflow)?;
        Ok(Portfolio {
            net_lots: NetLots::new(lots),
            cash: self.money(i128::from(portfolio.cash.amount().get()) + cash)?,
        })
    }

    fn payoff(
        &self,
        contract: OptionContract,
        close: PriceTicks,
    ) -> Result<Money, AccountingError> {
        let intrinsic_ticks = close.get().saturating_sub(contract.strike().get());
        Ok(self
            .instrument
            .option_notional(intrinsic_ticks, contract.size())?)
    }

    fn stock_value(&self, lots: NetLots, close: PriceTicks) -> Result<i128, AccountingError> {
        let tick = self.instrument.tick_value_per_share.get();
        let scaled = (i128::from(lots.get()) * i128::from(close.get()))
            .checked_mul(i128::from(self.instrument.lot_size.get()))
            .and_then(|value| value.checked_mul(i128::from(tick.amount().get())))
            .ok_or(DomainError::Overflow)?;
        let scale = i128::from(LotSize::MICROS_PER_SHARE);
        let whole = scaled / scale;
        let remainder = scaled % scale;
        Ok(whole + i128::from(remainder.abs() * 2 >= scale) * scaled.signum())
    }

    fn currency(&self) -> CurrencyId {
        self.instrument.tick_value_per_share.get().currency()
    }

    fn amount(&self, money: Money) -> Result<i128, AccountingError> {
        if money.currency() != self.currency() {
            return Err(DomainError::CurrencyMismatch.into());
        }
        Ok(i128::from(money.amount().get()))
    }

    fn money(&self, amount: i128) -> Result<Money, AccountingError> {
        let amount = i64::try_from(amount).map_err(|_| DomainError::Overflow)?;
        Ok(Money::new(self.currency(), CashUnits::new(amount)))
    }
}

fn agent_index(agent: AgentId) -> usize {
    agent.get() as usize
}

fn totals(portfolios: &[Portfolio]) -> (i128, i128) {
    portfolios.iter().fold((0, 0), |(cash, lots), portfolio| {
        (
            cash + i128::from(portfolio.cash.amount().get()),
            lots + i128::from(portfolio.net_lots.get()),
        )
    })
}

#[cfg(test)]
mod tests;
