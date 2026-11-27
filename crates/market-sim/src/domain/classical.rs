//! Daily short-call baselines, independent of order limits and auction execution.
//! API follows Daniil's #14 prototype; financial controls: issue #10 shared Python fixtures.
//! Leland: Nguyen & Pergamenshchikov, arXiv:1505.02546v1, (5)--(7).
//! Projection rule: Imaki et al., arXiv:2103.01775v1, (12)--(13).
//! AI assistance: ChatGPT; see docs/classical-strategies.md for assumptions and checks.

use super::hedging::{CallExposure, HedgingPolicy, Observation, PolicyError, TargetPosition};
use super::units::{DomainError, FundingRate, Shares, TradingDays};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClassicalMethod {
    BlackScholes,
    Leland,
    Band { width: BandWidth },
}

/// Half-width as a fraction of the short call's underlying exposure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandWidth(f64);

impl BandWidth {
    pub fn new(width: f64) -> Result<Self, DomainError> {
        if !(0.0..=1.0).contains(&width) {
            return Err(DomainError::InvalidValue("hedge band width"));
        }
        Ok(Self(width))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassicalPolicy {
    method: ClassicalMethod,
    continuous_rate: f64,
    days_per_year: TradingDays,
}

impl ClassicalPolicy {
    pub fn new(
        method: ClassicalMethod,
        annual_rate: FundingRate,
        days_per_year: TradingDays,
    ) -> Result<Self, DomainError> {
        let days = f64::from(days_per_year.get());
        if days == 0.0 || annual_rate.get() / days <= -1.0 {
            return Err(DomainError::InvalidValue(
                "pricing funding factor/day count",
            ));
        }
        Ok(Self {
            method,
            continuous_rate: days * (annual_rate.get() / days).ln_1p(),
            days_per_year,
        })
    }

    fn target(&self, observation: &Observation) -> Result<Shares, PolicyError> {
        let zero = Shares::new(0.0).expect("zero shares");
        let Some(call) = observation.call else {
            return Ok(zero);
        };
        if call.time_to_expiry.get() == 0 {
            return Ok(zero);
        }
        let CallExposure::Short(size) = call.underlying_exposure else {
            // Only the short-call seller cohort is specified by the research protocol.
            return Err(PolicyError::EvaluationFailed);
        };
        let exposure = f64::from(size.get());
        let sigma = observation.annualized_volatility.get();
        let days = f64::from(self.days_per_year.get());
        let c = observation.commission.get();
        let volatility = if self.method == ClassicalMethod::Leland && c != 0.0 && sigma != 0.0 {
            sigma.sqrt() * (sigma + c * (8.0 * days / std::f64::consts::PI).sqrt()).sqrt()
        } else {
            sigma
        };
        let years = f64::from(call.time_to_expiry.get()) / days;
        let discounted_strike = (-self.continuous_rate * years).exp();
        let total_volatility = volatility * years.sqrt();
        if !discounted_strike.is_finite() || !total_volatility.is_finite() {
            return Err(PolicyError::EvaluationFailed);
        }
        let m = call.moneyness.get();
        let delta = if total_volatility == 0.0 {
            if m > discounted_strike {
                1.0
            } else if m < discounted_strike {
                0.0
            } else {
                0.5
            }
        } else {
            let d1 =
                (m.ln() + self.continuous_rate * years) / total_volatility + total_volatility / 2.0;
            0.5 * libm::erfc(-d1 * std::f64::consts::FRAC_1_SQRT_2)
        };
        let center = exposure * delta;
        let position = if let ClassicalMethod::Band { width } = self.method {
            let half_width = exposure * width.0;
            observation.shares_held.get().clamp(
                (center - half_width).max(0.0),
                (center + half_width).min(exposure),
            )
        } else {
            center
        };
        Ok(Shares::new(position).expect("finite target bounded by short-call exposure"))
    }
}

impl HedgingPolicy for ClassicalPolicy {
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError> {
        if observations.len() != targets.len() {
            return Err(PolicyError::BatchLengthMismatch);
        }
        for (observation, target) in observations.iter().zip(targets) {
            target.target_shares = self.target(observation)?;
        }
        Ok(())
    }
}
