//! Issue #10 target fixtures checked by Python, passed through the common Rust executor.
//! AI assistance: ChatGPT; controls distinguish intention, capped order and partial fill.

use market_sim::domain::classical::{BandWidth, ClassicalMethod, ClassicalPolicy};
use market_sim::domain::hedging::{
    CallExposure, CallObservation, HedgingPolicy, Moneyness, Observation, PolicyError,
    TargetPosition,
};
use market_sim::domain::market::auction::clear_auction;
use market_sim::domain::market::{InstrumentSpec, Order, OrderLimits, Side, TickValuePerShare};
use market_sim::domain::units::{
    AgentId, AnnualizedVolatility, CashUnits, ContractSize, CurrencyId, FundingRate, LotSize, Lots,
    MarketStep, Money, NetLots, OneWayCommissionRate, PriceTicks, Shares, TradingDays,
};
use serde_json::Value;

#[test]
fn all_classical_targets_use_the_same_limits_and_actual_partial_fills() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/classical-strategies.json"
    ))
    .unwrap();
    let execution = &fixture["execution"];
    let number = |key: &str| execution[key].as_u64().unwrap();
    let instrument = InstrumentSpec {
        tick_value_per_share: TickValuePerShare::new(Money::new(
            CurrencyId::new(*b"USD").unwrap(),
            CashUnits::new(number("tick_value_per_share") as i64),
        ))
        .unwrap(),
        lot_size: LotSize::new(number("lot_size_micros") as u32).unwrap(),
    };
    let limits = OrderLimits::new(
        Lots::new(number("max_order_lots")).unwrap(),
        Lots::new(number("max_abs_position_lots")).unwrap(),
    )
    .unwrap();
    let price = PriceTicks::new(number("price_ticks")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let method = match case["method"].as_str().unwrap() {
            "bs" => ClassicalMethod::BlackScholes,
            "leland" => ClassicalMethod::Leland,
            "band" => ClassicalMethod::Band {
                width: BandWidth::new(case["band_width"].as_f64().unwrap()).unwrap(),
            },
            _ => panic!("unknown policy fixture"),
        };
        let policy = ClassicalPolicy::new(
            method,
            FundingRate::new(case["annual_funding_rate"].as_f64().unwrap()).unwrap(),
            TradingDays::new(case["days_per_year"].as_u64().unwrap() as u32),
        )
        .unwrap();
        let observation = Observation {
            as_of: MarketStep::new(0),
            shares_held: Shares::new(case["shares_held"].as_f64().unwrap()).unwrap(),
            annualized_volatility: AnnualizedVolatility::new(
                case["annualized_volatility"].as_f64().unwrap(),
            )
            .unwrap(),
            commission: OneWayCommissionRate::new(case["commission"].as_f64().unwrap()).unwrap(),
            call: Some(CallObservation {
                moneyness: Moneyness::new(case["moneyness"].as_f64().unwrap()).unwrap(),
                time_to_expiry: TradingDays::new(case["remaining_days"].as_u64().unwrap() as u32),
                underlying_exposure: CallExposure::Short(
                    ContractSize::new(case["size_shares"].as_u64().unwrap() as u32).unwrap(),
                ),
            }),
        };
        let mut targets = [TargetPosition {
            target_shares: Shares::new(0.0).unwrap(),
        }];
        policy.targets(&[observation], &mut targets).unwrap();
        assert!(
            (targets[0].target_shares.get() - case["expected_target"].as_f64().unwrap()).abs()
                < 1e-12,
            "{case}"
        );
        let expected_order = case["expected_signed_order_lots"].as_i64().unwrap();
        let order = limits
            .hedge_order(
                instrument,
                AgentId::new(0),
                NetLots::new(case["shares_held"].as_i64().unwrap()),
                targets[0],
                price,
            )
            .unwrap();
        if expected_order == 0 {
            assert!(order.is_none());
            assert_eq!(case["expected_signed_fill_lots"].as_i64().unwrap(), 0);
            continue;
        }
        let order = order.unwrap();
        assert_eq!(order.lots().get(), expected_order.unsigned_abs());
        assert_eq!(
            order.side(),
            if expected_order > 0 {
                Side::Buy
            } else {
                Side::Sell
            }
        );
        let counterparty = Order::new(
            AgentId::new(1),
            if expected_order > 0 {
                Side::Sell
            } else {
                Side::Buy
            },
            Lots::new(number("counterparty_lots")).unwrap(),
            price,
        );
        let auction = clear_auction(MarketStep::new(1), price, &[order, counterparty]).unwrap();
        let fill = auction
            .fills
            .iter()
            .find(|fill| fill.buyer() == AgentId::new(0) || fill.seller() == AgentId::new(0))
            .unwrap();
        let sign = if fill.buyer() == AgentId::new(0) {
            1
        } else {
            -1
        };
        assert_eq!(
            sign * fill.lots().get() as i64,
            case["expected_signed_fill_lots"].as_i64().unwrap()
        );
        assert_eq!(fill.price(), price);
    }
}

#[test]
fn missing_calls_bad_batches_and_invalid_parameters_are_explicit() {
    let method = ClassicalMethod::BlackScholes;
    let rate = FundingRate::new(0.0).unwrap();
    assert!(ClassicalPolicy::new(method, rate, TradingDays::new(0)).is_err());
    assert!(
        ClassicalPolicy::new(
            method,
            FundingRate::new(-252.0).unwrap(),
            TradingDays::new(252)
        )
        .is_err()
    );
    assert!(BandWidth::new(f64::NAN).is_err());
    assert!(BandWidth::new(-0.1).is_err());
    assert!(BandWidth::new(1.1).is_err());
    let policy = ClassicalPolicy::new(method, rate, TradingDays::new(252)).unwrap();
    let mut obs = Observation {
        as_of: MarketStep::new(0),
        shares_held: Shares::new(50.0).unwrap(),
        annualized_volatility: AnnualizedVolatility::new(0.2).unwrap(),
        commission: OneWayCommissionRate::new(0.001).unwrap(),
        call: None,
    };
    let mut targets = [TargetPosition {
        target_shares: obs.shares_held,
    }];
    policy.targets(&[obs], &mut targets).unwrap();
    assert_eq!(targets[0].target_shares.get(), 0.0);
    assert_eq!(
        policy.targets(&[obs], &mut []),
        Err(PolicyError::BatchLengthMismatch)
    );
    obs.call = Some(CallObservation {
        moneyness: Moneyness::new(1.0).unwrap(),
        time_to_expiry: TradingDays::new(20),
        underlying_exposure: CallExposure::Long(ContractSize::new(100).unwrap()),
    });
    assert_eq!(
        policy.targets(&[obs], &mut targets),
        Err(PolicyError::EvaluationFailed)
    );
    obs.call = Some(CallObservation {
        moneyness: Moneyness::new(1.0).unwrap(),
        time_to_expiry: TradingDays::new(1000),
        underlying_exposure: CallExposure::Short(ContractSize::new(100).unwrap()),
    });
    obs.annualized_volatility = AnnualizedVolatility::new(f64::MAX).unwrap();
    assert_eq!(
        policy.targets(&[obs], &mut targets),
        Err(PolicyError::EvaluationFailed)
    );
}
