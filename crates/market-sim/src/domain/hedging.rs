use super::units::{
    AnnualizedVolatility, ContractSize, DomainError, MarketStep, OneWayCommissionRate, Shares,
    TradingDays,
};

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Moneyness(f64);

impl Moneyness {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(DomainError::InvalidValue("Moneyness"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub as_of: MarketStep,
    pub shares_held: Shares,
    pub annualized_volatility: AnnualizedVolatility,
    pub commission: OneWayCommissionRate,
    pub call: Option<CallObservation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CallObservation {
    pub moneyness: Moneyness,
    pub time_to_expiry: TradingDays,
    pub underlying_exposure: CallExposure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallExposure {
    Long(ContractSize),
    Short(ContractSize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetPosition {
    pub target_shares: Shares,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    BatchLengthMismatch,
    EvaluationFailed,
}

pub trait HedgingPolicy {
    /// Write one absolute position per observation in the same order.
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moneyness_is_a_positive_finite_ratio() {
        assert_eq!(Moneyness::new(0.8).unwrap().get(), 0.8);
        assert!(Moneyness::new(0.0).is_err());
        assert!(Moneyness::new(f64::INFINITY).is_err());
    }
}
