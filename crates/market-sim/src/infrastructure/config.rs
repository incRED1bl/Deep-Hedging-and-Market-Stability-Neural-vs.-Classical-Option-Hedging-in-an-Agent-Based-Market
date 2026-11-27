use std::fmt;

use serde::Deserialize;

use crate::application::scenario::{CallSchedule, CashDecimalPlaces, PopulationCounts, Scenario};
use crate::domain::market::{InstrumentSpec, OrderLimits, TickValuePerShare};
use crate::domain::units::{
    AnnualizedVolatility, CashUnits, ContractSize, CurrencyId, DomainError, FundingRate, LotSize,
    Lots, Money, OneWayCommissionRate, PriceTicks, TradingDays,
};

#[derive(Debug)]
pub enum ConfigError {
    Json(serde_json::Error),
    Value(DomainError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::Value(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Value(error) => Some(error),
        }
    }
}

impl From<DomainError> for ConfigError {
    fn from(error: DomainError) -> Self {
        Self::Value(error)
    }
}

/// Decode a scenario from a JSON document and check its financial units.
pub fn parse_scenario(input: &str) -> Result<Scenario, ConfigError> {
    let raw: ScenarioJson = serde_json::from_str(input).map_err(ConfigError::Json)?;
    if raw.schema_version != 1 {
        return Err(DomainError::InvalidValue("scenario schema version").into());
    }
    let currency_bytes = raw
        .instrument
        .quote_currency
        .as_bytes()
        .try_into()
        .map_err(|_| DomainError::InvalidValue("CurrencyId"))?;
    let currency = CurrencyId::new(currency_bytes)?;
    let tick_value = Money::new(
        currency,
        CashUnits::new(raw.instrument.tick_value_per_share),
    );
    let scenario = Scenario {
        seed: raw.seed,
        instrument: InstrumentSpec {
            tick_value_per_share: TickValuePerShare::new(tick_value)?,
            lot_size: LotSize::new(raw.instrument.lot_size_micros)?,
        },
        cash_decimal_places: CashDecimalPlaces::new(raw.instrument.cash_decimal_places)?,
        initial_close: PriceTicks::new(raw.instrument.initial_close_ticks)?,
        initial_volatility: AnnualizedVolatility::new(raw.initial_annualized_volatility)?,
        population: PopulationCounts {
            fundamentalists: raw.population.fundamentalists,
            trend_traders: raw.population.trend_traders,
            noise_traders: raw.population.noise_traders,
            hedgers: raw.population.hedgers,
        },
        burn_in: TradingDays::new(raw.burn_in_days),
        observed_days: TradingDays::new(raw.observed_days),
        trading_days_per_year: TradingDays::new(raw.trading_days_per_year),
        commission: OneWayCommissionRate::new(raw.one_way_commission)?,
        funding_rate: FundingRate::new(raw.annual_funding_rate)?,
        limits: OrderLimits::new(
            Lots::new(raw.limits.max_order_lots)?,
            Lots::new(raw.limits.max_abs_position_lots)?,
        )?,
        call: CallSchedule {
            size: ContractSize::new(raw.call.size_shares)?,
            maturity: TradingDays::new(raw.call.maturity_days),
        },
    };
    scenario.validate()?;
    Ok(scenario)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioJson {
    schema_version: u32,
    seed: u64,
    instrument: InstrumentJson,
    initial_annualized_volatility: f64,
    population: PopulationJson,
    burn_in_days: u32,
    observed_days: u32,
    trading_days_per_year: u32,
    one_way_commission: f64,
    annual_funding_rate: f64,
    limits: LimitsJson,
    call: CallJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentJson {
    quote_currency: String,
    cash_decimal_places: u8,
    tick_value_per_share: i64,
    lot_size_micros: u32,
    initial_close_ticks: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PopulationJson {
    fundamentalists: u32,
    trend_traders: u32,
    noise_traders: u32,
    hedgers: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsJson {
    max_order_lots: u64,
    max_abs_position_lots: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallJson {
    size_shares: u32,
    maturity_days: u32,
}
