use std::collections::BTreeSet;

use super::super::{OptionContract, PremiumPerShare};
use super::{AccountingError, Ledger};
use crate::domain::units::{
    AgentId, ContractId, ContractSize, DomainError, MarketStep, PriceTicks, TradingDays,
};

pub const CALL_TENOR: TradingDays = TradingDays::new(20);

/// The caller supplies buyer/seller pairs in stable order. Consecutive sellers
/// use consecutive phases, so forty pairs place two issues on each of twenty days.
#[derive(Debug)]
pub struct Rotation {
    pairs: Vec<(AgentId, AgentId)>,
    size: ContractSize,
    tenor: TradingDays,
}

impl Rotation {
    pub fn new(
        pairs: Vec<(AgentId, AgentId)>,
        size: ContractSize,
        tenor: TradingDays,
    ) -> Result<Self, AccountingError> {
        if tenor.get() == 0 {
            return Err(DomainError::InvalidValue("call tenor").into());
        }
        let mut sellers = BTreeSet::new();
        for &(buyer, seller) in &pairs {
            if buyer == seller {
                return Err(DomainError::SameCounterparty.into());
            }
            if !sellers.insert(seller) {
                return Err(AccountingError::ActiveSeller(seller));
            }
        }
        Ok(Self { pairs, size, tenor })
    }

    /// Issue at the current close after settling that day's expiries.
    /// The phase is the position in the pair list modulo the call tenor.
    pub fn issue_due(
        &self,
        ledger: &mut Ledger,
        step: MarketStep,
        close: PriceTicks,
        premium: PremiumPerShare,
    ) -> Result<usize, AccountingError> {
        let phase = (step.get() % self.tenor.get()) as usize;
        let due = self
            .pairs
            .iter()
            .enumerate()
            .skip(phase)
            .step_by(self.tenor.get() as usize);
        if phase >= self.pairs.len() {
            return Ok(0);
        }
        let expires_at = MarketStep::new(
            step.get()
                .checked_add(self.tenor.get())
                .ok_or(DomainError::Overflow)?,
        );
        // At most 2^32 distinct sellers are possible in one rotation.
        let first_id = u64::from(step.get()) * self.pairs.len() as u64;
        ledger.transact(|ledger| {
            let mut issued = 0;
            for (index, &(buyer, seller)) in due {
                let contract = OptionContract::new(
                    ContractId::new(first_id + index as u64),
                    buyer,
                    seller,
                    self.size,
                    close,
                    premium,
                    step,
                    expires_at,
                )
                .expect("rotation already validated counterparties and expiry");
                ledger.issue_call(contract)?;
                issued += 1;
            }
            Ok(issued)
        })
    }
}
