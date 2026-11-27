//! Shared, hand-calculated issue #5 fixtures; actual auction fills and domain cash rules.
//! AI assistance: ChatGPT. The independent Python oracle consumes the same JSON.

use market_sim::domain::market::auction::clear_auction;
use market_sim::domain::market::{InstrumentSpec, Order, Side, TickValuePerShare};
use market_sim::domain::units::{
    AgentId, CashUnits, ContractSize, CurrencyId, FundingRate, LotSize, Lots, MarketStep, Money,
    OneWayCommissionRate, PriceTicks, TradingDays,
};
use serde_json::Value;

#[test]
fn cash_flows_from_actual_partial_fills_match_the_python_reference_fixtures() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/finance-reference.json"
    ))
    .unwrap();
    let currency = CurrencyId::new(*b"USD").unwrap();
    let money = |amount| Money::new(currency, CashUnits::new(amount));
    let instrument = InstrumentSpec {
        tick_value_per_share: TickValuePerShare::new(money(
            fixture["instrument"]["tick_value_per_share"]
                .as_i64()
                .unwrap(),
        ))
        .unwrap(),
        lot_size: LotSize::new(fixture["instrument"]["lot_size_micros"].as_u64().unwrap() as u32)
            .unwrap(),
    };
    for case in fixture["accounting_cases"].as_array().unwrap() {
        let call = &case["call"];
        let at = |key: &str| case[key].as_i64().unwrap();
        let price = PriceTicks::new(case["fills"][0]["price_ticks"].as_u64().unwrap()).unwrap();
        // A three-share buy intention receives only the one share offered by its counterparty.
        let orders = [
            Order::new(AgentId::new(0), Side::Buy, Lots::new(3).unwrap(), price),
            Order::new(AgentId::new(1), Side::Sell, Lots::new(1).unwrap(), price),
        ];
        let auction = clear_auction(MarketStep::new(1), price, &orders).unwrap();
        let fill = auction.fills[0];
        assert_eq!(auction.close.get(), at("first_close_ticks") as u64);
        assert_eq!(fill.lots().get(), 1);
        let premium = call["premium_per_share_units"].as_i64().unwrap()
            * call["size_shares"].as_i64().unwrap();
        let opening_cash = money(at("opening_cash_units") + premium);
        let notional = instrument.notional(fill.price(), fill.lots()).unwrap();
        let fee = OneWayCommissionRate::new(case["commission"].as_f64().unwrap())
            .unwrap()
            .apply(notional)
            .unwrap();
        let interest = FundingRate::new(case["annual_funding_rate"].as_f64().unwrap())
            .unwrap()
            .daily_interest(opening_cash, TradingDays::new(252))
            .unwrap();
        let first_cash = opening_cash
            .checked_sub(notional)
            .unwrap()
            .checked_sub(fee)
            .unwrap()
            .checked_add(interest)
            .unwrap();
        // The hedger does not trade on day two. Other participants set the expiry close.
        let expiry_price = PriceTicks::new(at("close_ticks") as u64).unwrap();
        let expiry_orders = [
            Order::new(
                AgentId::new(2),
                Side::Buy,
                Lots::new(1).unwrap(),
                expiry_price,
            ),
            Order::new(
                AgentId::new(3),
                Side::Sell,
                Lots::new(1).unwrap(),
                expiry_price,
            ),
        ];
        let expiry = clear_auction(MarketStep::new(2), auction.close, &expiry_orders).unwrap();
        assert_eq!(call["expires_at"].as_u64().unwrap(), 2);
        let close = expiry.close.get();
        let expiry_interest = FundingRate::new(case["annual_funding_rate"].as_f64().unwrap())
            .unwrap()
            .daily_interest(first_cash, TradingDays::new(252))
            .unwrap();
        let payoff = instrument
            .option_notional(
                close.saturating_sub(call["strike_ticks"].as_u64().unwrap()),
                ContractSize::new(call["size_shares"].as_u64().unwrap() as u32).unwrap(),
            )
            .unwrap();
        let cash = first_cash
            .checked_add(expiry_interest)
            .unwrap()
            .checked_sub(payoff)
            .unwrap();
        assert_eq!(cash.amount().get(), at("expected_cash_units"));
        let opening_value = at("opening_cash_units")
            + instrument
                .notional(
                    PriceTicks::new(at("opening_price_ticks") as u64).unwrap(),
                    Lots::new(at("opening_lots") as u64).unwrap(),
                )
                .unwrap()
                .amount()
                .get();
        let stock = instrument
            .notional(
                PriceTicks::new(close).unwrap(),
                Lots::new(at("opening_lots") as u64 + fill.lots().get()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            cash.amount().get() + stock.amount().get() - opening_value,
            at("expected_pnl_units")
        );
    }
}

#[test]
fn binary_float_rate_boundaries_match_the_shared_python_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/finance-reference.json"
    ))
    .unwrap();
    let currency = CurrencyId::new(*b"USD").unwrap();
    for case in fixture["rate_cases"].as_array().unwrap() {
        let rate = case["rate"].as_f64().unwrap();
        let amount = Money::new(
            currency,
            CashUnits::new(case["notional_units"].as_i64().unwrap()),
        );
        let expected = case["expected_units"].as_i64().unwrap();
        assert_eq!(
            FundingRate::new(rate)
                .unwrap()
                .daily_interest(amount, TradingDays::new(1))
                .unwrap()
                .amount()
                .get(),
            expected
        );
        if rate >= 0.0 {
            assert_eq!(
                OneWayCommissionRate::new(rate)
                    .unwrap()
                    .apply(amount)
                    .unwrap()
                    .amount()
                    .get(),
                expected
            );
        }
    }
}
