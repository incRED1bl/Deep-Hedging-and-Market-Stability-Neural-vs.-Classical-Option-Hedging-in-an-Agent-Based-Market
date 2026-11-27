use std::collections::BTreeMap;

use market_sim::domain::{
    accounting::{
        OptionContract, Portfolio, PremiumPerShare,
        ledger::{AccountingError, CALL_TENOR, Ledger, PostingKind, Rotation},
    },
    market::{Fill, InstrumentSpec, Order, Side, TickValuePerShare, auction::clear_auction},
    units::{
        AgentId, CashUnits, ContractId, ContractSize, CurrencyId, DomainError, FundingRate,
        LotSize, Lots, MarketStep, Money, NetLots, OneWayCommissionRate, PriceTicks, TradingDays,
    },
};

const HEDGER: AgentId = AgentId::new(0);
const OPTION_BUYER: AgentId = AgentId::new(1);
const STOCK_SELLER: AgentId = AgentId::new(2);
const FEES: AgentId = AgentId::new(3);
const FUNDING: AgentId = AgentId::new(4);
const OPEN: MarketStep = MarketStep::new(0);
const TRADE: MarketStep = MarketStep::new(1);

fn instrument() -> InstrumentSpec {
    InstrumentSpec {
        tick_value_per_share: TickValuePerShare::new(cash(1)).unwrap(),
        lot_size: LotSize::new(1_000_000).unwrap(),
    }
}

fn cash(cents: i64) -> Money {
    Money::new(CurrencyId::new(*b"USD").unwrap(), CashUnits::new(cents))
}

fn portfolio(net_lots: i64, cents: i64) -> Portfolio {
    Portfolio {
        net_lots: NetLots::new(net_lots),
        cash: cash(cents),
    }
}

fn price(ticks: u64) -> PriceTicks {
    PriceTicks::new(ticks).unwrap()
}

#[test]
fn real_partial_fill_replays_the_hand_calculated_itm_atm_and_otm_pnl() {
    let opening = [
        portfolio(2, 20_000),
        portfolio(0, 100_000),
        portfolio(10, -20_000),
        portfolio(0, 0),
        portfolio(0, 0),
    ];
    let strike = price(10_000);
    let expiry = MarketStep::new(20);
    let auction = clear_auction(
        TRADE,
        strike,
        &[
            Order::new(HEDGER, Side::Buy, Lots::new(3).unwrap(), strike),
            Order::new(STOCK_SELLER, Side::Sell, Lots::new(1).unwrap(), strike),
        ],
    )
    .unwrap();
    assert_eq!(auction.close, strike);
    assert_eq!(auction.fills.len(), 1);

    for (terminal, payoff, hedger_pnl, buyer_pnl) in [
        (11_000, 2_000, 1_511, 1_600),
        (10_000, 0, 511, -400),
        (9_000, 0, -2_489, -400),
    ] {
        let mut ledger = Ledger::new(instrument(), &opening, FEES, FUNDING).unwrap();
        let contract = OptionContract::new(
            ContractId::new(42),
            OPTION_BUYER,
            HEDGER,
            ContractSize::new(2).unwrap(),
            strike,
            PremiumPerShare::new(cash(250)).unwrap(),
            OPEN,
            expiry,
        )
        .unwrap();
        ledger.issue_call(contract).unwrap();
        let opening_cash = ledger.portfolios().to_vec();
        for &fill in &auction.fills {
            ledger
                .book_fill(TRADE, fill, OneWayCommissionRate::new(0.001).unwrap())
                .unwrap();
        }
        assert_eq!(ledger.portfolio(HEDGER), Some(&portfolio(3, 10_490)));
        assert_eq!(ledger.portfolio(STOCK_SELLER), Some(&portfolio(9, -10_010)));
        assert_eq!(ledger.portfolio(FEES), Some(&portfolio(0, 20)));

        ledger
            .accrue_funding(
                TRADE,
                &opening_cash,
                FundingRate::new(0.252).unwrap(),
                TradingDays::new(252),
            )
            .unwrap();
        assert_eq!(ledger.portfolio(HEDGER), Some(&portfolio(3, 10_511)));
        assert_eq!(ledger.portfolio(OPTION_BUYER), Some(&portfolio(0, 99_600)));
        assert_eq!(ledger.portfolio(STOCK_SELLER), Some(&portfolio(9, -10_030)));
        assert_eq!(ledger.portfolio(FUNDING), Some(&portfolio(0, -101)));

        let close = price(terminal);
        let unsettled_postings = ledger.postings().len();
        assert_eq!(
            ledger.settle_expiring(MarketStep::new(expiry.get() + 1), close),
            Err(AccountingError::OverdueContract(contract.id()))
        );
        assert_eq!(ledger.postings().len(), unsettled_postings);
        assert_eq!(ledger.settle_expiring(expiry, close).unwrap(), 1);
        assert_eq!(ledger.settle_expiring(expiry, close).unwrap(), 0);
        assert!(matches!(
            ledger.postings().last().unwrap().kind,
            PostingKind::OptionSettlement { payoff: paid, .. } if paid == cash(payoff)
        ));
        ledger.check_conservation().unwrap();

        let journal = ledger.postings().to_vec();
        let rebuilt = Ledger::replay(instrument(), FEES, FUNDING, &journal).unwrap();
        assert_eq!(rebuilt.portfolios(), ledger.portfolios());
        assert_eq!(rebuilt.postings(), journal);
        assert_eq!(rebuilt.active_call(HEDGER), None);
        let marks = BTreeMap::new();
        assert_eq!(
            rebuilt.pnl(HEDGER, strike, close, &marks).unwrap(),
            cash(hedger_pnl)
        );
        assert_eq!(
            rebuilt.pnl(OPTION_BUYER, strike, close, &marks).unwrap(),
            cash(buyer_pnl)
        );
    }
}

#[test]
fn forty_hedgers_roll_two_contracts_each_day_and_keep_losses_and_positions() {
    let option_buyer = AgentId::new(40);
    let fees = AgentId::new(41);
    let funding = AgentId::new(42);
    let mut opening = vec![portfolio(2, 100); 40];
    opening.extend([portfolio(0, 100), portfolio(0, 0), portfolio(0, 0)]);
    let mut ledger = Ledger::new(instrument(), &opening, fees, funding).unwrap();
    let rotation = Rotation::new(
        (0..40).map(|id| (option_buyer, AgentId::new(id))).collect(),
        ContractSize::new(1).unwrap(),
        CALL_TENOR,
    )
    .unwrap();

    for day in 0..=22 {
        let step = MarketStep::new(day);
        let close = price(10_000 + u64::from(day));
        let settled = if day < CALL_TENOR.get() { 0 } else { 2 };
        assert_eq!(ledger.settle_expiring(step, close).unwrap(), settled);
        assert_eq!(
            rotation
                .issue_due(
                    &mut ledger,
                    step,
                    close,
                    PremiumPerShare::new(cash(3)).unwrap()
                )
                .unwrap(),
            2
        );

        for seller in [AgentId::new(day % 20), AgentId::new(day % 20 + 20)] {
            let contract = ledger.active_call(seller).unwrap();
            assert_eq!(contract.issued_at(), step);
            assert_eq!(
                contract.expires_at(),
                MarketStep::new(day + CALL_TENOR.get())
            );
            assert_eq!(contract.strike(), close);
            let remaining_cash = if day < CALL_TENOR.get() { 103 } else { 86 };
            assert_eq!(
                ledger.portfolio(seller),
                Some(&portfolio(2, remaining_cash))
            );
        }
    }
    assert_eq!(ledger.active_calls().count(), 40);
    ledger.check_conservation().unwrap();
}

#[test]
fn funding_retry_preserves_balances_and_postings_before_and_after_replay() {
    let opening = [
        portfolio(2, 10_000),
        portfolio(-2, -5_000),
        portfolio(0, 0),
        portfolio(0, 0),
        portfolio(0, 0),
    ];
    let spec = instrument();
    let next_day = MarketStep::new(2);
    let mut ledger = Ledger::new(spec, &opening, FEES, FUNDING).unwrap();
    let rate = FundingRate::new(0.02).unwrap();
    let days = TradingDays::new(2);
    let fill = Fill::new(HEDGER, OPTION_BUYER, Lots::new(1).unwrap(), price(15)).unwrap();
    let commission = OneWayCommissionRate::new(0.1).unwrap();
    ledger.accrue_funding(TRADE, &opening, rate, days).unwrap();
    let journal = ledger.postings().to_vec();
    let funded = ledger.portfolios().to_vec();
    assert_eq!(ledger.portfolio(HEDGER), Some(&portfolio(2, 10_100)));
    assert_eq!(ledger.portfolio(OPTION_BUYER), Some(&portfolio(-2, -5_050)));
    assert_eq!(ledger.portfolio(FUNDING), Some(&portfolio(0, -50)));
    assert_eq!(journal.len(), opening.len() + 2);

    let mut replayed = Ledger::replay(spec, FEES, FUNDING, &journal).unwrap();
    for ledger in [&mut ledger, &mut replayed] {
        ledger.accrue_funding(TRADE, &opening, rate, days).unwrap();
        assert_eq!(ledger.portfolios(), funded);
        assert_eq!(ledger.postings(), journal);

        ledger
            .accrue_funding(next_day, &funded, rate, days)
            .unwrap();
        assert_eq!(ledger.portfolio(HEDGER), Some(&portfolio(2, 10_201)));
        assert_eq!(ledger.portfolio(OPTION_BUYER), Some(&portfolio(-2, -5_101)));
        assert_eq!(ledger.portfolio(FUNDING), Some(&portfolio(0, -100)));
        assert_eq!(ledger.postings().len(), journal.len() + 2);
        assert_eq!(ledger.check_conservation(), Ok(()));

        ledger
            .book_fill(MarketStep::new(3), fill, commission)
            .unwrap();
        let after_fill = ledger.portfolios().to_vec();
        assert_eq!(
            ledger.accrue_funding(next_day, &funded, rate, days),
            Err(AccountingError::BackdatedPosting(next_day))
        );
        assert_eq!(ledger.portfolios(), after_fill);
        assert_eq!(ledger.postings().len(), journal.len() + 3);
    }
    let replayed = Ledger::replay(spec, FEES, FUNDING, ledger.postings()).unwrap();
    assert_eq!(replayed.portfolios(), ledger.portfolios());
}

#[test]
fn funding_calculation_overflow_is_reported_and_reverts_earlier_transfers() {
    let opening = [
        portfolio(2, 100),
        portfolio(0, i64::MAX / 2),
        portfolio(0, 0),
        portfolio(0, 0),
        portfolio(0, 0),
    ];
    let mut ledger = Ledger::new(instrument(), &opening, FEES, FUNDING).unwrap();
    let starting_postings = ledger.postings().to_vec();
    // The first participant earns 300 cash units. The second calculation overflows.
    let error = ledger
        .accrue_funding(
            TRADE,
            &opening,
            FundingRate::new(3.0).unwrap(),
            TradingDays::new(1),
        )
        .unwrap_err();

    assert_eq!(error, AccountingError::Domain(DomainError::Overflow));
    assert_eq!(error.to_string(), "arithmetic overflow");
    assert_eq!(ledger.portfolios(), opening);
    assert_eq!(ledger.postings(), starting_postings);
    ledger.check_conservation().unwrap();
}
