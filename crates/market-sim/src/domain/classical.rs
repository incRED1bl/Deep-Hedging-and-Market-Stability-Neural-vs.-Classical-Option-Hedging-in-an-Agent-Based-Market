use super::hedging::{CallExposure, HedgingPolicy, Observation, PolicyError, TargetPosition};
use super::units::{DomainError, FundingRate, Shares, TradingDays};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClassicalMethod {
    BlackScholes,
    Leland,
    Band { width: BandWidth },
}

/// No-trade half-width as a fraction of the contract's share exposure.
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
    annual_rate: FundingRate,
    days_per_year: TradingDays,
}

impl ClassicalPolicy {
    pub fn new(
        method: ClassicalMethod,
        annual_rate: FundingRate,
        days_per_year: TradingDays,
    ) -> Result<Self, DomainError> {
        if days_per_year.get() == 0 {
            return Err(DomainError::InvalidValue("pricing day count"));
        }
        Ok(Self {
            method,
            annual_rate,
            days_per_year,
        })
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
            let Some(call) = observation.call else {
                target.target_shares = Shares::new(0.0).expect("zero shares");
                continue;
            };
            let exposure = hedge_exposure(call.underlying_exposure);
            let annualized = observation.annualized_volatility.get();
            let width = match self.method {
                ClassicalMethod::Band { width } => width.0,
                _ => 0.0,
            };
            let volatility = if self.method == ClassicalMethod::Leland {
                let adjustment = observation.commission.get()
                    * (8.0 * f64::from(self.days_per_year.get()) / std::f64::consts::PI).sqrt();
                annualized.sqrt() * (annualized + adjustment).sqrt()
            } else {
                annualized
            };
            let years = f64::from(call.time_to_expiry.get()) / f64::from(self.days_per_year.get());
            let delta = call_terms(
                call.moneyness.get(),
                years,
                self.annual_rate.get(),
                volatility,
            )
            .0;
            let center = exposure * delta;
            let band = exposure.abs() * width;
            let min = center - band;
            let max = center + band;
            let position = observation.shares_held.get().clamp(min, max);
            target.target_shares = Shares::new(position).expect("finite delta and exposure");
        }
        Ok(())
    }
}

pub(crate) fn hedge_exposure(exposure: CallExposure) -> f64 {
    match exposure {
        CallExposure::Short(size) => f64::from(size.get()),
        CallExposure::Long(size) => -f64::from(size.get()),
    }
}

/// A no-dividend European call, expressed as a ratio of the strike.
pub(crate) fn call_terms(moneyness: f64, years: f64, rate: f64, volatility: f64) -> (f64, f64) {
    let total_volatility = volatility * years.sqrt();
    let forward_log = moneyness.ln() + rate * years;
    let log_discount = -rate * years;
    if total_volatility == 0.0 {
        let delta = if forward_log > 0.0 {
            1.0
        } else if forward_log < 0.0 {
            0.0
        } else {
            0.5
        };
        return (delta, (moneyness - log_discount.exp()).max(0.0));
    }
    let standardized = forward_log / total_volatility;
    let half_volatility = total_volatility / 2.0;
    let delta = normal_cdf(standardized + half_volatility);
    let lower = standardized - half_volatility;
    let discounted_probability = (log_discount + log_normal_cdf(lower)).exp();
    let price_ratio = moneyness * delta - discounted_probability;
    (delta, price_ratio.clamp(0.0, moneyness))
}

fn normal_cdf(value: f64) -> f64 {
    0.5 * libm::erfc(-value * std::f64::consts::FRAC_1_SQRT_2)
}

fn log_normal_cdf(value: f64) -> f64 {
    if value >= -10.0 {
        return normal_cdf(value).ln();
    }
    // Mills ratio expansion prevents underflow before applying the discount.
    let square = value * value;
    let inv = 1.0 / square;
    let ratio = 1.0 + inv * (-1.0 + inv * (3.0 + inv * (-15.0 + inv * (105.0 - 945.0 * inv))));
    -0.5 * square - (-value).ln() - 0.5 * std::f64::consts::TAU.ln() + ratio.ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_call_uses_discounted_intrinsic_value() {
        assert_eq!(call_terms(0.8, 0.0, 0.05, 0.4), (0.0, 0.0));
        assert_eq!(call_terms(1.0, 0.0, 0.05, 0.4), (0.5, 0.0));
        assert_eq!(call_terms(1.25, 1.0, 0.0, 0.0), (1.0, 0.25));
    }

    #[test]
    fn extreme_discount_retains_the_nonzero_tail_term() {
        let (delta, ratio) = call_terms(1.0, 1000.0, -1.0, 2.0_f64.sqrt());
        assert!((delta - 0.5).abs() < 1e-12);
        assert!((ratio - 0.491_083_833_055_729).abs() < 1e-12);
    }
}
