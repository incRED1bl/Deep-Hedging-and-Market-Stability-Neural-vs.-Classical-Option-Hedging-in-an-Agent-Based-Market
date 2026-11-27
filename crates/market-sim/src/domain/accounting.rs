pub mod ledger;

use super::units::{
    AgentId, ContractId, ContractSize, DomainError, MarketStep, Money, NetLots, PriceTicks,
    rounded_cash,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Portfolio {
    pub net_lots: NetLots,
    pub cash: Money,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PremiumPerShare(Money);

impl PremiumPerShare {
    pub fn new(value: Money) -> Result<Self, DomainError> {
        if value.amount().get() < 0 {
            return Err(DomainError::InvalidValue("PremiumPerShare"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> Money {
        self.0
    }

    /// Apply the booked per-share premium to the whole contract.
    pub fn total(self, size: ContractSize) -> Result<Money, DomainError> {
        let numerator = i128::from(self.0.amount().get()) * i128::from(size.get());
        rounded_cash(self.0.currency(), numerator, 1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OptionContract {
    id: ContractId,
    buyer: AgentId,
    seller: AgentId,
    size: ContractSize,
    strike: PriceTicks,
    premium_per_share: PremiumPerShare,
    issued_at: MarketStep,
    expires_at: MarketStep,
}

impl OptionContract {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ContractId,
        buyer: AgentId,
        seller: AgentId,
        size: ContractSize,
        strike: PriceTicks,
        premium_per_share: PremiumPerShare,
        issued_at: MarketStep,
        expires_at: MarketStep,
    ) -> Result<Self, DomainError> {
        if buyer == seller {
            return Err(DomainError::SameCounterparty);
        }
        if expires_at <= issued_at {
            return Err(DomainError::InvalidExpiry);
        }
        Ok(Self {
            id,
            buyer,
            seller,
            size,
            strike,
            premium_per_share,
            issued_at,
            expires_at,
        })
    }

    pub const fn id(self) -> ContractId {
        self.id
    }

    pub const fn buyer(self) -> AgentId {
        self.buyer
    }

    pub const fn seller(self) -> AgentId {
        self.seller
    }

    pub const fn size(self) -> ContractSize {
        self.size
    }

    pub const fn strike(self) -> PriceTicks {
        self.strike
    }

    pub const fn premium_per_share(self) -> PremiumPerShare {
        self.premium_per_share
    }

    pub const fn issued_at(self) -> MarketStep {
        self.issued_at
    }

    pub const fn expires_at(self) -> MarketStep {
        self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::units::{CashUnits, CurrencyId};

    #[test]
    fn options_check_counterparties_expiry_and_premium() {
        let currency = CurrencyId::new(*b"USD").unwrap();
        let money = |units| Money::new(currency, CashUnits::new(units));
        assert!(PremiumPerShare::new(money(-1)).is_err());
        let premium = PremiumPerShare::new(money(0)).unwrap();
        assert_eq!(premium.get(), money(0));
        let buyer = AgentId::new(0);
        let seller = AgentId::new(1);
        let start = MarketStep::new(5);
        let build = |seller, end| {
            OptionContract::new(
                ContractId::new(2),
                buyer,
                seller,
                ContractSize::new(2).unwrap(),
                PriceTicks::new(100).unwrap(),
                premium,
                start,
                end,
            )
        };
        assert!(build(seller, MarketStep::new(6)).is_ok());
        assert_eq!(
            build(buyer, MarketStep::new(6)),
            Err(DomainError::SameCounterparty)
        );
        assert_eq!(build(seller, start), Err(DomainError::InvalidExpiry));
        assert_eq!(
            PremiumPerShare::new(money(i64::MAX))
                .unwrap()
                .total(ContractSize::new(2).unwrap()),
            Err(DomainError::Overflow),
        );
    }
}
