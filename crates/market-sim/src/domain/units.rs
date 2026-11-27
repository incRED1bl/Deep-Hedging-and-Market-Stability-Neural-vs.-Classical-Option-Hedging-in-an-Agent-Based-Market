use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainError {
    InvalidValue(&'static str),
    SameCounterparty,
    InvalidExpiry,
    CurrencyMismatch,
    Overflow,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue(value) => write!(formatter, "invalid {value}"),
            Self::SameCounterparty => formatter.write_str("counterparties must be distinct"),
            Self::InvalidExpiry => formatter.write_str("expiry must be after issuance"),
            Self::CurrencyMismatch => formatter.write_str("cash currencies differ"),
            Self::Overflow => formatter.write_str("arithmetic overflow"),
        }
    }
}

impl std::error::Error for DomainError {}

macro_rules! integer_unit {
    ($name:ident, $raw:ty) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($raw);

        impl $name {
            pub const fn new(value: $raw) -> Self {
                Self(value)
            }

            pub const fn get(self) -> $raw {
                self.0
            }
        }
    };
}

macro_rules! positive_integer_unit {
    ($name:ident, $raw:ty) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($raw);

        impl $name {
            pub const fn new(value: $raw) -> Result<Self, DomainError> {
                if value == 0 {
                    return Err(DomainError::InvalidValue(stringify!($name)));
                }
                Ok(Self(value))
            }

            pub const fn get(self) -> $raw {
                self.0
            }
        }
    };
}

integer_unit!(AgentId, u32);
integer_unit!(ContractId, u64);
integer_unit!(MarketStep, u32);
integer_unit!(TradingDays, u32);
integer_unit!(CashUnits, i64);
integer_unit!(NetLots, i64);

positive_integer_unit!(Lots, u64);
positive_integer_unit!(PriceTicks, u64);
positive_integer_unit!(LotSize, u32);
positive_integer_unit!(ContractSize, u32);

impl LotSize {
    /// The integer scale used to record shares per lot.
    pub const MICROS_PER_SHARE: u32 = 1_000_000;

    /// Convert the stored millionths of one share to a share count.
    pub fn shares(self) -> f64 {
        f64::from(self.get()) / f64::from(Self::MICROS_PER_SHARE)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Shares(f64);

impl Shares {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() {
            return Err(DomainError::InvalidValue("Shares"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }

    pub(crate) fn from_lots(lots: NetLots, lot_size: LotSize) -> Self {
        let micros = i128::from(lots.get()) * i128::from(lot_size.get());
        Self(micros as f64 / f64::from(LotSize::MICROS_PER_SHARE))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct AnnualizedVolatility(f64);

impl AnnualizedVolatility {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() || value < 0.0 {
            return Err(DomainError::InvalidValue("AnnualizedVolatility"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct GrossReturn(f64);

impl GrossReturn {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(DomainError::InvalidValue("GrossReturn"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CurrencyId([u8; 3]);

impl CurrencyId {
    pub fn new(code: [u8; 3]) -> Result<Self, DomainError> {
        if !code.iter().all(u8::is_ascii_uppercase) {
            return Err(DomainError::InvalidValue("CurrencyId"));
        }
        Ok(Self(code))
    }

    pub const fn get(self) -> [u8; 3] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Money {
    currency: CurrencyId,
    amount: CashUnits,
}

impl Money {
    pub const fn new(currency: CurrencyId, amount: CashUnits) -> Self {
        Self { currency, amount }
    }

    pub const fn currency(self) -> CurrencyId {
        self.currency
    }

    pub const fn amount(self) -> CashUnits {
        self.amount
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, DomainError> {
        self.ensure_currency(rhs)?;
        let amount = self.amount.get().checked_add(rhs.amount.get());
        Ok(Self::new(
            self.currency,
            CashUnits::new(amount.ok_or(DomainError::Overflow)?),
        ))
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, DomainError> {
        self.ensure_currency(rhs)?;
        let amount = self.amount.get().checked_sub(rhs.amount.get());
        Ok(Self::new(
            self.currency,
            CashUnits::new(amount.ok_or(DomainError::Overflow)?),
        ))
    }

    fn ensure_currency(self, rhs: Self) -> Result<(), DomainError> {
        if self.currency != rhs.currency {
            return Err(DomainError::CurrencyMismatch);
        }
        Ok(())
    }
}

const RATE_SCALE: i64 = 1_000_000_000;

fn scaled_rate(value: f64, name: &'static str) -> Result<i64, DomainError> {
    let scaled = (value * RATE_SCALE as f64).round();
    // The exclusive upper bound is exactly representable as f64.
    if !((i64::MIN as f64)..(-(i64::MIN as f64))).contains(&scaled) {
        return Err(DomainError::InvalidValue(name));
    }
    Ok(scaled as i64)
}

/// A nonnegative fraction charged to each counterparty on a fill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneWayCommissionRate(i64);

impl OneWayCommissionRate {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if value < 0.0 {
            return Err(DomainError::InvalidValue("OneWayCommissionRate"));
        }
        Ok(Self(scaled_rate(value, "OneWayCommissionRate")?))
    }

    pub fn get(self) -> f64 {
        self.0 as f64 / RATE_SCALE as f64
    }

    pub fn apply(self, notional: Money) -> Result<Money, DomainError> {
        if notional.amount.get() < 0 {
            return Err(DomainError::InvalidValue("trade notional"));
        }
        rate_cash(self.0, notional, i128::from(RATE_SCALE))
    }
}

/// A signed, simple annual cash funding rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FundingRate(i64);

impl FundingRate {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        Ok(Self(scaled_rate(value, "FundingRate")?))
    }

    pub fn get(self) -> f64 {
        self.0 as f64 / RATE_SCALE as f64
    }

    pub fn daily_interest(
        self,
        opening_cash: Money,
        trading_days_per_year: TradingDays,
    ) -> Result<Money, DomainError> {
        if trading_days_per_year.get() == 0 {
            return Err(DomainError::InvalidValue("trading days per year"));
        }
        let denominator = i128::from(RATE_SCALE) * i128::from(trading_days_per_year.get());
        rate_cash(self.0, opening_cash, denominator)
    }
}

fn rate_cash(rate: i64, cash: Money, denominator: i128) -> Result<Money, DomainError> {
    let numerator = i128::from(rate) * i128::from(cash.amount.get());
    rounded_cash(cash.currency, numerator, denominator)
}

/// Round a signed cash amount to the nearest unit, with ties away from zero.
pub(crate) fn rounded_cash(
    currency: CurrencyId,
    numerator: i128,
    denominator: i128,
) -> Result<Money, DomainError> {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = if remainder.abs() * 2 >= denominator {
        quotient + numerator.signum()
    } else {
        quotient
    };
    let amount = i64::try_from(rounded).map_err(|_| DomainError::Overflow)?;
    Ok(Money::new(currency, CashUnits::new(amount)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cash(value: i64) -> Money {
        Money::new(CurrencyId::new(*b"USD").unwrap(), CashUnits::new(value))
    }

    #[test]
    fn numeric_invariants() {
        assert!(Lots::new(0).is_err());
        assert!(PriceTicks::new(0).is_err());
        assert!(LotSize::new(0).is_err());
        assert_eq!(LotSize::new(100_000).unwrap().shares(), 0.1);
        assert!(ContractSize::new(0).is_err());
        assert!(Shares::new(f64::NAN).is_err());
        assert!(AnnualizedVolatility::new(-0.1).is_err());
        assert_eq!(AnnualizedVolatility::new(0.2).unwrap().get(), 0.2);
        assert_eq!(
            GrossReturn::new(0.0).unwrap_err().to_string(),
            "invalid GrossReturn"
        );
        assert_eq!(GrossReturn::new(1.01).unwrap().get(), 1.01);
        assert!(CurrencyId::new(*b"USd").is_err());
        assert_eq!(cash(-37).currency().get(), *b"USD");
    }

    #[test]
    fn cash_operations_preserve_currency_and_check_bounds() {
        assert_eq!(cash(35).checked_add(cash(-5)), Ok(cash(30)));
        assert_eq!(cash(35).checked_sub(cash(50)), Ok(cash(-15)));
        let euro = Money::new(CurrencyId::new(*b"EUR").unwrap(), CashUnits::new(1));
        assert_eq!(
            cash(0).checked_add(euro),
            Err(DomainError::CurrencyMismatch)
        );
        assert_eq!(
            cash(0).checked_sub(euro),
            Err(DomainError::CurrencyMismatch)
        );
        assert_eq!(
            cash(i64::MAX).checked_add(cash(1)),
            Err(DomainError::Overflow)
        );
        assert_eq!(
            cash(i64::MIN).checked_sub(cash(1)),
            Err(DomainError::Overflow)
        );
        assert_eq!(
            DomainError::SameCounterparty.to_string(),
            "counterparties must be distinct"
        );
        assert_eq!(
            DomainError::InvalidExpiry.to_string(),
            "expiry must be after issuance"
        );
        assert_eq!(
            DomainError::CurrencyMismatch.to_string(),
            "cash currencies differ"
        );
        assert_eq!(DomainError::Overflow.to_string(), "arithmetic overflow");
    }

    #[test]
    fn commission_and_funding_round_signed_cash_once() {
        assert!(OneWayCommissionRate::new(-0.1).is_err());
        assert!(FundingRate::new(f64::INFINITY).is_err());
        assert!(OneWayCommissionRate::new(f64::INFINITY).is_err());
        let fee = OneWayCommissionRate::new(0.005).unwrap();
        assert_eq!(fee.get(), 0.005);
        assert_eq!(fee.apply(cash(99)), Ok(cash(0)));
        assert_eq!(fee.apply(cash(100)), Ok(cash(1)));
        assert!(fee.apply(cash(-100)).is_err());
        let funding = FundingRate::new(0.252).unwrap();
        let days = TradingDays::new(252);
        assert_eq!(funding.get(), 0.252);
        assert_eq!(funding.daily_interest(cash(500), days), Ok(cash(1)));
        assert_eq!(funding.daily_interest(cash(-500), days), Ok(cash(-1)));
        assert_eq!(
            FundingRate::new(-0.252)
                .unwrap()
                .daily_interest(cash(500), days),
            Ok(cash(-1))
        );
        assert!(
            funding
                .daily_interest(cash(500), TradingDays::new(0))
                .is_err()
        );
    }
}
