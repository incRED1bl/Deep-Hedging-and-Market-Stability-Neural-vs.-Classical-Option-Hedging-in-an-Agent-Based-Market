use std::cell::RefCell;

use market_sim::application::market_day::{
    MarketDay, MarketDayError, MarketRecord, PositionAggregate,
};
use market_sim::application::ports::{ShockAddress, ShockKind, ShockSource};
use market_sim::application::scenario::{PopulationCounts, Scenario};
use market_sim::domain::agents::AgentState;
use market_sim::domain::hedging::TargetPosition;
use market_sim::domain::market::OrderLimits;
use market_sim::domain::market::auction::AuctionError;
use market_sim::domain::units::{
    AgentId, DomainError, Lots, MarketStep, NetLots, Shares, TradingDays,
};
use market_sim::infrastructure::config::parse_scenario;
use market_sim::infrastructure::shocks::SeededShocks;

fn scenario() -> Scenario {
    parse_scenario(include_str!("fixtures/simulation-config.json")).unwrap()
}

fn small_scenario() -> Scenario {
    let mut scenario = scenario();
    scenario.population = PopulationCounts {
        fundamentalists: 1,
        trend_traders: 1,
        noise_traders: 1,
        hedgers: 1,
    };
    scenario.limits = OrderLimits::new(Lots::new(8).unwrap(), Lots::new(20).unwrap()).unwrap();
    scenario
}

fn target(shares: f64) -> TargetPosition {
    TargetPosition {
        target_shares: Shares::new(shares).unwrap(),
    }
}

#[derive(Default)]
struct FixedShocks {
    addresses: RefCell<Vec<ShockAddress>>,
}

impl ShockSource for FixedShocks {
    fn draw(&self, address: ShockAddress) -> u64 {
        self.addresses.borrow_mut().push(address);
        match (address.kind, address.agent.map(AgentId::get)) {
            (ShockKind::AgentSignal, Some(0)) => u64::MAX,
            (ShockKind::AgentSignal, Some(2)) => 0,
            _ => 1 << 63,
        }
    }
}

fn finish_day(
    market: &MarketDay,
    shocks: &impl ShockSource,
    positions: &mut [NetLots],
    targets: &[TargetPosition],
) -> (MarketDay, MarketRecord) {
    let prepared = market.prepare(shocks, positions, targets).unwrap();
    let state = prepared.state();
    for fill in prepared.fills() {
        let buyer = fill.buyer().get() as usize;
        let seller = fill.seller().get() as usize;
        let lots = i64::try_from(fill.lots().get()).unwrap();
        positions[buyer] = NetLots::new(positions[buyer].get() + lots);
        positions[seller] = NetLots::new(positions[seller].get() - lots);
    }
    let completed = prepared.finish(positions).unwrap();
    assert_eq!(completed.1.state, state);
    completed
}

#[test]
fn hedge_orders_change_the_clearing_price_and_only_fills_change_holdings() {
    let market = MarketDay::new(&small_scenario()).unwrap();
    let mut neutral_positions = [NetLots::new(0); 4];
    let mut buying_positions = neutral_positions;
    let mut selling_positions = neutral_positions;
    let neutral_shocks = FixedShocks::default();
    let buying_shocks = FixedShocks::default();

    let (_, neutral) = finish_day(
        &market,
        &neutral_shocks,
        &mut neutral_positions,
        &[target(0.0)],
    );
    let (next, buying) = finish_day(
        &market,
        &buying_shocks,
        &mut buying_positions,
        &[target(8.0)],
    );
    let (_, selling) = finish_day(
        &market,
        &FixedShocks::default(),
        &mut selling_positions,
        &[target(-8.0)],
    );

    assert_eq!(neutral.state.close.get(), 10_000);
    assert_eq!(buying.state.close.get(), 10_081);
    assert_eq!(selling.state.close.get(), 9_880);
    assert_eq!(buying.traded_lots, 3);
    assert_eq!(buying.fills.len(), 1);
    assert_eq!(buying.fills[0].buyer(), AgentId::new(3));
    assert_eq!(buying.fills[0].seller(), AgentId::new(2));
    assert_eq!(buying_positions[3], NetLots::new(3));
    assert_eq!(
        buying.positions.hedgers,
        PositionAggregate {
            net_lots: 3,
            gross_lots: 3
        }
    );
    assert_eq!(
        buying.positions.noise_traders,
        PositionAggregate {
            net_lots: -3,
            gross_lots: 3
        }
    );
    assert_eq!(
        buying.positions.fundamentalists,
        PositionAggregate::default()
    );
    assert_eq!(buying.positions.trend_traders, PositionAggregate::default());
    assert_eq!(neutral_shocks.addresses, buying_shocks.addresses);
    assert_eq!(buying_shocks.addresses.borrow().len(), 4);
    assert!(
        buying_shocks
            .addresses
            .borrow()
            .iter()
            .all(|address| address.step.get() == 0)
    );
    assert_eq!(market.state().step.get(), 0);

    let hedge = AgentId::new(3);
    let prepared = next
        .prepare(&buying_shocks, &buying_positions, &[target(3.0)])
        .unwrap();
    assert!(
        prepared
            .fills()
            .iter()
            .all(|fill| fill.buyer() != hedge && fill.seller() != hedge)
    );
    assert!(
        buying_shocks.addresses.borrow()[4..]
            .iter()
            .all(|address| address.step.get() == 1)
    );
}

#[test]
fn volatility_updates_with_only_the_just_completed_auction() {
    let market = MarketDay::new(&small_scenario()).unwrap();
    let start = market.state();
    let mut positions = [NetLots::new(0); 4];
    let (next, first) = finish_day(
        &market,
        &FixedShocks::default(),
        &mut positions,
        &[target(8.0)],
    );
    let expected_return = 10_081.0_f64 / 10_000.0;
    let expected_volatility =
        (0.94 * 0.20_f64.powi(2) + 0.06 * 252.0 * expected_return.ln().powi(2)).sqrt();

    assert_eq!(first.state.gross_return.get(), expected_return);
    assert!((first.state.annualized_volatility.get() - expected_volatility).abs() < 1e-12);
    assert_eq!(next.state(), first.state);
    assert_eq!(market.state(), start);
    assert_eq!(start.gross_return.get(), 1.0);
    assert_eq!(start.annualized_volatility.get(), 0.20);

    let (following, second) = finish_day(
        &next,
        &FixedShocks::default(),
        &mut positions,
        &[target(-8.0)],
    );
    assert_eq!(second.state.step, MarketStep::new(2));
    assert_eq!(next.state(), first.state);
    let AgentState::TrendTrader { signal, .. } = following.population().agents()[1] else {
        panic!("expected a trend trader");
    };
    assert!((signal - 0.30 * 1.05 * expected_return.ln()).abs() < 1e-12);
}

#[test]
fn a_fundamental_move_without_crossing_orders_carries_the_previous_close() {
    struct FundamentalShock;

    impl ShockSource for FundamentalShock {
        fn draw(&self, address: ShockAddress) -> u64 {
            match address.kind {
                ShockKind::Fundamental => u64::MAX,
                ShockKind::AgentSignal => 1 << 63,
            }
        }
    }

    let scenario = Scenario {
        population: PopulationCounts {
            fundamentalists: 1,
            trend_traders: 0,
            noise_traders: 0,
            hedgers: 0,
        },
        ..scenario()
    };
    let market = MarketDay::new(&scenario).unwrap();
    let (next, record) = finish_day(&market, &FundamentalShock, &mut [NetLots::new(0)], &[]);

    let AgentState::Fundamentalist { signal, .. } = next.population().agents()[0] else {
        panic!("expected a fundamentalist");
    };
    assert!(signal > 0.0);
    assert!(record.fills.is_empty());
    assert_eq!(record.state.close, scenario.initial_close);
    assert_eq!(record.state.gross_return.get(), 1.0);
}

#[test]
fn configured_population_has_individual_states_and_replays_full_horizon() {
    let run = |scenario: Scenario| {
        let mut market = MarketDay::new(&scenario).unwrap();
        assert_eq!(market.population().len(), 200);
        assert!(!market.population().is_empty());
        assert_eq!(
            market
                .population()
                .hedgers()
                .map(AgentId::get)
                .collect::<Vec<_>>(),
            (160..200).collect::<Vec<_>>()
        );

        let mut positions = vec![NetLots::new(0); 200];
        let mut records = Vec::new();
        for _ in 0..scenario.burn_in.get() + scenario.observed_days.get() {
            let targets: Vec<_> = market
                .population()
                .hedgers()
                .map(|id| TargetPosition {
                    target_shares: scenario.instrument.shares(positions[id.get() as usize]),
                })
                .collect();
            let (next, record) = finish_day(&market, &SeededShocks, &mut positions, &targets);
            let cohort_lots = record.positions;
            assert_eq!(
                cohort_lots.fundamentalists.net_lots
                    + cohort_lots.trend_traders.net_lots
                    + cohort_lots.noise_traders.net_lots
                    + cohort_lots.hedgers.net_lots,
                0
            );
            assert!(record.state.annualized_volatility.get().is_finite());
            assert!(record.state.gross_return.get().is_finite());
            market = next;
            records.push(record);
        }
        assert!(records.iter().any(|record| !record.fills.is_empty()));
        assert!(
            records
                .iter()
                .any(|record| record.state.close != scenario.initial_close)
        );
        (market, records)
    };

    let original = scenario();
    let (market, records) = run(original);
    assert_eq!(records.len(), 2_500);
    let agents = market.population().agents();
    assert!(
        agents[..60]
            .iter()
            .all(|agent| matches!(agent, AgentState::Fundamentalist { .. }))
    );
    assert!(
        agents[60..110]
            .iter()
            .all(|agent| matches!(agent, AgentState::TrendTrader { .. }))
    );
    assert!(
        agents[110..160]
            .iter()
            .all(|agent| matches!(agent, AgentState::NoiseTrader { .. }))
    );
    assert!(agents[160..].iter().all(|agent| matches!(
        agent,
        AgentState::Hedger {
            target: Some(_),
            ..
        }
    )));
    for (expected, agent) in (0..200).zip(agents) {
        assert_eq!(agent.id().get(), expected);
    }
    let (
        AgentState::Fundamentalist { signal: first, .. },
        AgentState::Fundamentalist { signal: second, .. },
    ) = (agents[0], agents[1])
    else {
        panic!("fundamentalists at the beginning of the population");
    };
    assert_ne!(first, second);
    assert_eq!((market, records.clone()), run(original));
    let mut other_seed = original;
    other_seed.seed += 1;
    assert_ne!(records, run(other_seed).1);
}

#[test]
fn seeded_draws_depend_on_the_entire_address() {
    let first = ShockAddress {
        market_seed: 17,
        step: MarketStep::new(29),
        agent: Some(AgentId::new(0)),
        kind: ShockKind::AgentSignal,
        draw_index: 0,
    };
    let variants = [
        ShockAddress {
            market_seed: 19,
            ..first
        },
        ShockAddress {
            step: MarketStep::new(30),
            ..first
        },
        ShockAddress {
            agent: None,
            ..first
        },
        ShockAddress {
            agent: Some(AgentId::new(1)),
            ..first
        },
        ShockAddress {
            kind: ShockKind::Fundamental,
            ..first
        },
        ShockAddress {
            draw_index: 1,
            ..first
        },
    ];
    for variant in variants {
        let original = SeededShocks.draw(first);
        assert_ne!(original, SeededShocks.draw(variant));
        assert_eq!(SeededShocks.draw(first), original);
    }
}

#[test]
fn inconsistent_inputs_leave_the_committed_day_available() {
    let mut scenario = small_scenario();
    scenario.observed_days = TradingDays::new(0);
    assert_eq!(
        MarketDay::new(&scenario),
        Err(MarketDayError::Domain(DomainError::InvalidValue(
            "scenario duration or day count"
        )))
    );

    let market = MarketDay::new(&small_scenario()).unwrap();
    let start = market.state();
    let valid = [NetLots::new(0); 4];
    let shocks = FixedShocks::default();
    assert_eq!(
        market.prepare(&shocks, &valid[..3], &[target(0.0)]),
        Err(MarketDayError::PositionCount {
            expected: 4,
            actual: 3
        })
    );
    assert_eq!(
        market.prepare(&shocks, &valid, &[]),
        Err(MarketDayError::HedgeTargetCount {
            expected: 1,
            actual: 0
        })
    );
    let mut invalid = valid;
    invalid[0] = NetLots::new(21);
    assert_eq!(
        market.prepare(&shocks, &invalid, &[target(0.0)]),
        Err(MarketDayError::Domain(DomainError::InvalidValue(
            "opening position"
        )))
    );
    let valid_day = market.prepare(&shocks, &valid, &[target(0.0)]).unwrap();
    assert_eq!(
        valid_day.finish(&[]),
        Err(MarketDayError::PositionCount {
            expected: 4,
            actual: 0
        })
    );
    assert_eq!(market.state(), start);
}

#[test]
fn auction_overflow_leaves_the_committed_day_available() {
    let mut scenario = small_scenario();
    scenario.population.hedgers = 2;
    scenario.limits = OrderLimits::new(
        Lots::new(u64::MAX).unwrap(),
        Lots::new(i64::MAX as u64).unwrap(),
    )
    .unwrap();
    let market = MarketDay::new(&scenario).unwrap();
    let start = market.state();
    let holdings = [
        NetLots::new(0),
        NetLots::new(0),
        NetLots::new(0),
        NetLots::new(-i64::MAX),
        NetLots::new(-i64::MAX),
    ];

    assert_eq!(
        market.prepare(&FixedShocks::default(), &holdings, &[target(f64::MAX); 2]),
        Err(MarketDayError::Auction(AuctionError::QuantityOverflow))
    );
    assert_eq!(market.state(), start);
}

#[test]
fn finish_rejects_unchanged_positions_after_fills() {
    let market = MarketDay::new(&small_scenario()).unwrap();
    let opening = [NetLots::new(0); 4];
    let prepared = market
        .prepare(&FixedShocks::default(), &opening, &[target(8.0)])
        .unwrap();

    assert_eq!(prepared.fills()[0].lots().get(), 3);
    assert_eq!(
        prepared.finish(&opening),
        Err(MarketDayError::ClosingPositionMismatch {
            agent: AgentId::new(2),
            expected: NetLots::new(-3),
            actual: NetLots::new(0),
        })
    );
    assert_eq!(market.state().step, MarketStep::new(0));
}

#[test]
fn finish_rejects_invented_positions_without_fills() {
    let scenario = Scenario {
        population: PopulationCounts {
            fundamentalists: 0,
            trend_traders: 0,
            noise_traders: 0,
            hedgers: 1,
        },
        ..small_scenario()
    };
    let market = MarketDay::new(&scenario).unwrap();
    let prepared = market
        .prepare(&SeededShocks, &[NetLots::new(7)], &[target(7.0)])
        .unwrap();

    assert!(prepared.fills().is_empty());
    assert_eq!(
        prepared.finish(&[NetLots::new(-2)]),
        Err(MarketDayError::ClosingPositionMismatch {
            agent: AgentId::new(0),
            expected: NetLots::new(7),
            actual: NetLots::new(-2),
        })
    );
    assert_eq!(market.state().step, MarketStep::new(0));
}

#[test]
fn finish_reconciles_fills_larger_than_the_signed_position_range() {
    let scenario = Scenario {
        population: PopulationCounts {
            fundamentalists: 0,
            trend_traders: 0,
            noise_traders: 0,
            hedgers: 2,
        },
        limits: OrderLimits::new(
            Lots::new(u64::MAX).unwrap(),
            Lots::new(i64::MAX as u64).unwrap(),
        )
        .unwrap(),
        ..small_scenario()
    };
    let opening = [NetLots::new(-i64::MAX), NetLots::new(i64::MAX)];
    let market = MarketDay::new(&scenario).unwrap();
    let prepared = market
        .prepare(
            &SeededShocks,
            &opening,
            &[target(f64::MAX), target(-f64::MAX)],
        )
        .unwrap();
    let (_, record) = prepared.finish(&[opening[1], opening[0]]).unwrap();

    assert_eq!(record.traded_lots, u64::MAX - 1);
    assert_eq!(record.positions.hedgers.net_lots, 0);
    assert_eq!(
        record.positions.hedgers.gross_lots,
        u128::from(u64::MAX - 1)
    );
}
