use crate::domain::market::{InstrumentSpec, OrderLimits};
use crate::domain::units::{
    AnnualizedVolatility, ContractSize, DomainError, FundingRate, OneWayCommissionRate, PriceTicks,
    TradingDays,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CashDecimalPlaces(u8);

impl CashDecimalPlaces {
    pub fn new(value: u8) -> Result<Self, DomainError> {
        if value > 9 {
            return Err(DomainError::InvalidValue("cash decimal places"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub fn units_per_currency(self) -> i64 {
        10_i64.pow(u32::from(self.0))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PopulationCounts {
    pub fundamentalists: u32,
    pub trend_traders: u32,
    pub noise_traders: u32,
    pub hedgers: u32,
}

impl PopulationCounts {
    pub fn total(self) -> Result<u32, DomainError> {
        self.fundamentalists
            .checked_add(self.trend_traders)
            .and_then(|total| total.checked_add(self.noise_traders))
            .and_then(|total| total.checked_add(self.hedgers))
            .ok_or(DomainError::Overflow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallSchedule {
    pub size: ContractSize,
    pub maturity: TradingDays,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scenario {
    pub seed: u64,
    pub instrument: InstrumentSpec,
    pub cash_decimal_places: CashDecimalPlaces,
    pub initial_close: PriceTicks,
    pub initial_volatility: AnnualizedVolatility,
    pub population: PopulationCounts,
    pub burn_in: TradingDays,
    pub observed_days: TradingDays,
    pub trading_days_per_year: TradingDays,
    pub commission: OneWayCommissionRate,
    pub funding_rate: FundingRate,
    pub limits: OrderLimits,
    pub call: CallSchedule,
}

impl Scenario {
    /// Ensure a typed configuration fits the simulator's time and agent address spaces.
    pub fn validate(&self) -> Result<(), DomainError> {
        self.burn_in
            .get()
            .checked_add(self.observed_days.get())
            .and_then(|end| end.checked_add(self.call.maturity.get()))
            .ok_or(DomainError::Overflow)?;
        let population = self.population.total()?;
        if population == 0 {
            return Err(DomainError::InvalidValue("population"));
        }
        // Reserve IDs following the trading population for fees, funding and calls.
        population.checked_add(3).ok_or(DomainError::Overflow)?;
        if self.observed_days.get() == 0
            || self.trading_days_per_year.get() == 0
            || self.call.maturity.get() == 0
        {
            return Err(DomainError::InvalidValue("scenario duration or day count"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::config::parse_scenario;

    #[test]
    fn scenario_reserves_agent_ids_and_steps() {
        let mut scenario =
            parse_scenario(include_str!("../../tests/fixtures/simulation-config.json")).unwrap();
        assert_eq!(scenario.cash_decimal_places.get(), 2);
        assert_eq!(scenario.cash_decimal_places.units_per_currency(), 100);
        assert!(CashDecimalPlaces::new(10).is_err());
        let mut no_days = scenario;
        no_days.observed_days = TradingDays::new(0);
        assert_eq!(
            no_days.validate(),
            Err(DomainError::InvalidValue("scenario duration or day count"))
        );
        scenario.burn_in = TradingDays::new(u32::MAX);
        assert_eq!(scenario.validate(), Err(DomainError::Overflow));
        scenario.burn_in = TradingDays::new(1);
        scenario.population = PopulationCounts {
            fundamentalists: 0,
            trend_traders: 0,
            noise_traders: 0,
            hedgers: 0,
        };
        assert_eq!(
            scenario.validate(),
            Err(DomainError::InvalidValue("population"))
        );
        scenario.population.hedgers = u32::MAX;
        assert_eq!(scenario.validate(), Err(DomainError::Overflow));
        scenario.population.noise_traders = 1;
        assert_eq!(scenario.validate(), Err(DomainError::Overflow));
    }
}
