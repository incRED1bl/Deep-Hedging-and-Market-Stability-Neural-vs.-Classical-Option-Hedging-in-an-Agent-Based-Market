use std::error::Error;

use market_sim::domain::accounting::{OptionContract, Portfolio, PremiumPerShare};
use market_sim::domain::hedging::{
    CallExposure, CallObservation, HedgingPolicy, Moneyness, Observation, PolicyError,
    TargetPosition,
};
use market_sim::domain::market::Fill;
use market_sim::domain::units::{
    AgentId, CashUnits, ContractId, DomainError, GrossReturn, Lots, MarketStep, Money, NetLots,
    PriceTicks, Shares, TradingDays,
};
use market_sim::infrastructure::config::{ConfigError, parse_scenario};

const EXAMPLE: &str = include_str!("fixtures/simulation-config.json");

struct FiveShares;

impl HedgingPolicy for FiveShares {
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError> {
        if observations.len() != targets.len() {
            return Err(PolicyError::BatchLengthMismatch);
        }
        targets.fill(TargetPosition {
            target_shares: Shares::new(5.0).unwrap(),
        });
        Ok(())
    }
}

#[test]
fn one_step_books_actual_fill_and_expiring_call_before_roll() {
    let scenario = parse_scenario(EXAMPLE).unwrap();
    let instrument = scenario.instrument;
    let currency = instrument.tick_value_per_share.get().currency();
    let cash = |cents| Money::new(currency, CashUnits::new(cents));
    let buyer = AgentId::new(1);
    let seller = AgentId::new(2);
    let open = MarketStep::new(19);
    let close = MarketStep::new(20);
    let strike = PriceTicks::new(9_900).unwrap();
    let contract = OptionContract::new(
        ContractId::new(7),
        seller,
        buyer,
        scenario.call.size,
        strike,
        PremiumPerShare::new(cash(60)).unwrap(),
        MarketStep::new(0),
        close,
    )
    .unwrap();
    let opening = Portfolio {
        net_lots: NetLots::new(2),
        cash: cash(20_000),
    };
    let observations = [Observation {
        as_of: open,
        shares_held: instrument.shares(opening.net_lots),
        annualized_volatility: scenario.initial_volatility,
        commission: scenario.commission,
        call: Some(CallObservation {
            moneyness: Moneyness::new(scenario.initial_close.get() as f64 / strike.get() as f64)
                .unwrap(),
            time_to_expiry: TradingDays::new(contract.expires_at().get() - open.get()),
            underlying_exposure: CallExposure::Short(contract.size()),
        }),
    }];
    let mut targets = [TargetPosition {
        target_shares: observations[0].shares_held,
    }];
    FiveShares.targets(&observations, &mut targets).unwrap();

    let order = scenario
        .limits
        .hedge_order(
            instrument,
            buyer,
            opening.net_lots,
            targets[0],
            scenario.initial_close,
        )
        .unwrap()
        .unwrap();
    assert_eq!(order.lots().get(), 3);

    // A counterparty supplies one lot. The remaining order quantity expires.
    let fill = Fill::new(order.agent(), seller, Lots::new(1).unwrap(), order.limit()).unwrap();
    let notional = instrument.notional(fill.price(), fill.lots()).unwrap();
    let fee = scenario.commission.apply(notional).unwrap();
    let buyer_pays = notional.checked_add(fee).unwrap();
    let seller_receives = notional.checked_sub(fee).unwrap();
    assert_eq!(buyer_pays, cash(10_010));
    assert_eq!(seller_receives, cash(9_990));
    assert_eq!(fee.checked_add(fee).unwrap(), cash(20));

    let funding = scenario
        .funding_rate
        .daily_interest(opening.cash, scenario.trading_days_per_year)
        .unwrap();
    let ticks_itm = fill.price().get().saturating_sub(contract.strike().get());
    let payoff = instrument
        .option_notional(ticks_itm, contract.size())
        .unwrap();
    let roll = OptionContract::new(
        ContractId::new(8),
        contract.buyer(),
        contract.seller(),
        contract.size(),
        fill.price(),
        PremiumPerShare::new(cash(100)).unwrap(),
        close,
        MarketStep::new(close.get() + scenario.call.maturity.get()),
    )
    .unwrap();
    assert_eq!(roll.id().get(), 8);
    assert_eq!(roll.issued_at(), close);
    assert_eq!(roll.expires_at().get(), 40);
    let premium = roll.premium_per_share().total(roll.size()).unwrap();
    assert_eq!(payoff, cash(100));
    assert_eq!(premium, cash(100));
    let updated = Portfolio {
        net_lots: NetLots::new(opening.net_lots.get() + i64::try_from(fill.lots().get()).unwrap()),
        cash: opening
            .cash
            .checked_sub(buyer_pays)
            .unwrap()
            .checked_add(funding)
            .unwrap()
            .checked_sub(payoff)
            .unwrap()
            .checked_add(premium)
            .unwrap(),
    };
    assert_eq!(instrument.shares(updated.net_lots).get(), 3.0);
    assert_eq!(updated.cash, cash(9_990));
    let gross_return =
        GrossReturn::new(fill.price().get() as f64 / scenario.initial_close.get() as f64).unwrap();
    assert_eq!(gross_return.get(), 1.0);
    assert!(GrossReturn::new(0.0).is_err());
}

#[test]
fn config_rejects_unknown_fields_bad_currency_and_inconsistent_durations() {
    let invalid_json = parse_scenario("{\"unknown\": 1}").unwrap_err();
    assert!(matches!(invalid_json, ConfigError::Json(_)));
    assert!(invalid_json.to_string().contains("unknown field"));
    assert!(invalid_json.source().is_some());

    let invalid_version =
        parse_scenario(&EXAMPLE.replace("\"schema_version\": 1", "\"schema_version\": 9"))
            .unwrap_err();
    assert!(matches!(
        invalid_version,
        ConfigError::Value(DomainError::InvalidValue("scenario schema version"))
    ));

    let invalid_currency = parse_scenario(&EXAMPLE.replace("\"USD\"", "\"US\"")).unwrap_err();
    assert!(matches!(
        invalid_currency,
        ConfigError::Value(DomainError::InvalidValue("CurrencyId"))
    ));
    assert_eq!(invalid_currency.to_string(), "invalid CurrencyId");
    assert!(invalid_currency.source().is_some());

    let excessive_limit = parse_scenario(&EXAMPLE.replace(
        "\"max_abs_position_lots\": 1000",
        "\"max_abs_position_lots\": 9223372036854775808",
    ))
    .unwrap_err();
    assert!(matches!(
        excessive_limit,
        ConfigError::Value(DomainError::InvalidValue("absolute position limit"))
    ));

    let invalid_days =
        parse_scenario(&EXAMPLE.replace("\"observed_days\": 2000", "\"observed_days\": 0"))
            .unwrap_err();
    assert!(matches!(
        invalid_days,
        ConfigError::Value(DomainError::InvalidValue(_))
    ));
}

#[test]
fn booked_fee_rounds_half_cash_unit_up() {
    let scenario = parse_scenario(EXAMPLE).unwrap();
    let currency = scenario.instrument.tick_value_per_share.get().currency();
    let notional = Money::new(currency, CashUnits::new(500));
    assert_eq!(
        scenario.commission.apply(notional).unwrap().amount().get(),
        1
    );
}
