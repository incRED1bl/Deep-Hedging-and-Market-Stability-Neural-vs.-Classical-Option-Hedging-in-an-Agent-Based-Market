use std::cell::{Cell, RefCell};
use std::fs;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use market_sim::application::ports::{ShockAddress, ShockSource};
use market_sim::application::simulation::{DayRecord, JournalSink, Simulation, SimulationError};
use market_sim::domain::accounting::ledger::{Ledger, PostingKind};
use market_sim::domain::classical::{BandWidth, ClassicalMethod, ClassicalPolicy};
use market_sim::domain::hedging::{HedgingPolicy, Observation, PolicyError, TargetPosition};
use market_sim::domain::units::{AgentId, FundingRate, TradingDays};
use market_sim::infrastructure::config::parse_scenario;
use market_sim::infrastructure::journal::JsonDirectoryJournal;
use market_sim::infrastructure::neural_policy::NeuralPolicy;
use market_sim::infrastructure::shocks::SeededShocks;

const SCENARIO: &str = include_str!("fixtures/integrated-scenario.json");
const MODEL: &str = include_str!("fixtures/prototype-model.json");

#[derive(Default)]
struct CapturedJournal(Vec<DayRecord>);

impl JournalSink for CapturedJournal {
    fn publish(&mut self, day: &DayRecord) -> io::Result<()> {
        self.0.push(day.clone());
        Ok(())
    }
}

#[derive(Default)]
struct CapturedShocks(RefCell<Vec<(ShockAddress, u64)>>);

impl ShockSource for CapturedShocks {
    fn draw(&self, address: ShockAddress) -> u64 {
        let draw = SeededShocks.draw(address);
        self.0.borrow_mut().push((address, draw));
        draw
    }
}

struct AuditedPolicy<'a> {
    policy: &'a dyn HedgingPolicy,
    observations: RefCell<Vec<Vec<Observation>>>,
}

impl HedgingPolicy for AuditedPolicy<'_> {
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError> {
        self.observations.borrow_mut().push(observations.to_vec());
        self.policy.targets(observations, targets)
    }
}

#[test]
fn all_four_policies_share_opening_state_and_shocks_and_replay_actual_holdings() {
    let mut scenario = parse_scenario(SCENARIO).unwrap();
    scenario.burn_in = TradingDays::new(2);
    scenario.observed_days = TradingDays::new(23);
    scenario.funding_rate = FundingRate::new(0.03).unwrap();
    let classical = |method| {
        ClassicalPolicy::new(
            method,
            scenario.funding_rate,
            scenario.trading_days_per_year,
        )
        .unwrap()
    };
    let bs = classical(ClassicalMethod::BlackScholes);
    let leland = classical(ClassicalMethod::Leland);
    let band = classical(ClassicalMethod::Band {
        width: BandWidth::new(0.05).unwrap(),
    });
    let neural = NeuralPolicy::from_json(MODEL, scenario.trading_days_per_year).unwrap();
    let strategies: [&dyn HedgingPolicy; 5] = [&bs, &leland, &band, &neural, &neural];

    let mut journals = Vec::new();
    let mut common_shocks = None;
    for policy in strategies {
        let audit = AuditedPolicy {
            policy,
            observations: RefCell::new(Vec::new()),
        };
        let shocks = CapturedShocks::default();
        let mut journal = CapturedJournal::default();
        let mut simulation = Simulation::new(scenario, &audit, &shocks, &mut journal).unwrap();
        simulation.run().unwrap();
        assert_eq!(simulation.state().step.get(), 25);
        simulation.ledger().check_conservation().unwrap();

        let run_shocks = shocks.0.into_inner();
        if let Some(expected) = &common_shocks {
            assert_eq!(expected, &run_shocks);
        } else {
            common_shocks = Some(run_shocks);
        }
        assert_eq!(journal.0.len(), 26);
        assert_eq!(
            journal
                .0
                .iter()
                .filter(|day| day.included_in_metrics)
                .count(),
            23
        );
        assert_eq!(audit.observations.borrow().len(), 25);
        let mut postings = Vec::new();
        for (day, observations) in journal.0.iter().zip(audit.observations.into_inner()) {
            postings.extend(&day.postings);
            let ledger = Ledger::replay(
                scenario.instrument,
                AgentId::new(200),
                AgentId::new(201),
                &postings,
            )
            .unwrap();
            assert_eq!(observations.len(), 40);
            for (index, observation) in observations.iter().enumerate() {
                let agent = AgentId::new(160 + index as u32);
                let held = ledger.portfolio(agent).unwrap().net_lots;
                assert_eq!(observation.as_of, day.market.state.step);
                assert_eq!(observation.shares_held, scenario.instrument.shares(held));
                assert_eq!(
                    observation.annualized_volatility,
                    day.market.state.annualized_volatility
                );
                assert_eq!(
                    observation.call.is_some(),
                    ledger.active_call(agent).is_some()
                );
                if let Some((call, contract)) = observation.call.zip(ledger.active_call(agent)) {
                    assert_eq!(
                        call.time_to_expiry.get(),
                        contract.expires_at().get() - observation.as_of.get()
                    );
                    assert_eq!(
                        call.moneyness.get(),
                        day.market.state.close.get() as f64 / contract.strike().get() as f64
                    );
                }
            }
            ledger.check_conservation().unwrap();
        }

        assert!(
            journal
                .0
                .iter()
                .skip(1)
                .any(|day| !day.market.fills.is_empty())
        );
        assert!(journal.0.iter().any(|day| {
            day.postings
                .iter()
                .any(|p| matches!(p.kind, PostingKind::Funding { .. }))
        }));
        assert!(journal.0.iter().any(|day| {
            day.postings
                .iter()
                .any(|p| matches!(p.kind, PostingKind::OptionSettlement { .. }))
        }));
        let last = journal.0.last().unwrap();
        assert_eq!(
            last.terminal.as_ref().unwrap().observed_hedger_pnl.len(),
            40
        );
        assert!(!last.terminal.as_ref().unwrap().live_call_marks.is_empty());
        assert!(
            last.postings
                .iter()
                .all(|posting| !matches!(posting.kind, PostingKind::OptionIssue { .. }))
        );
        journals.push(journal.0);
    }
    for journal in &journals[1..] {
        assert_eq!(journal.first(), journals[0].first());
    }
    assert_eq!(&journals[3], &journals[4]);
    assert_ne!(&journals[0], &journals[3]);
}

struct Retriable<'a> {
    policy: &'a dyn HedgingPolicy,
    fail: Cell<bool>,
}

impl HedgingPolicy for Retriable<'_> {
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError> {
        if self.fail.replace(false) {
            return Err(PolicyError::EvaluationFailed);
        }
        self.policy.targets(observations, targets)
    }
}

struct FailingJournal {
    fail_at_step: u32,
    days: Vec<DayRecord>,
}

impl JournalSink for FailingJournal {
    fn publish(&mut self, day: &DayRecord) -> io::Result<()> {
        if day.market.state.step.get() == self.fail_at_step {
            self.fail_at_step = u32::MAX;
            return Err(io::Error::other("injected write failure"));
        }
        self.days.push(day.clone());
        Ok(())
    }
}

#[test]
fn policy_and_journal_failures_leave_the_last_day_available_and_retryable() {
    let mut scenario = parse_scenario(SCENARIO).unwrap();
    scenario.burn_in = TradingDays::new(0);
    scenario.observed_days = TradingDays::new(2);
    let neural = NeuralPolicy::from_json(MODEL, scenario.trading_days_per_year).unwrap();
    let policy = Retriable {
        policy: &neural,
        fail: Cell::new(true),
    };
    let mut journal = FailingJournal {
        fail_at_step: 1,
        days: Vec::new(),
    };
    let mut simulation = Simulation::new(scenario, &policy, &SeededShocks, &mut journal).unwrap();
    let opening_postings = simulation.ledger().postings().to_vec();
    let error = simulation.advance().unwrap_err();
    assert!(matches!(error, SimulationError::Policy(_)));
    assert!(error.to_string().contains("hedging batch"));
    let error = simulation.advance().unwrap_err();
    assert!(matches!(error, SimulationError::Journal(_)));
    assert!(error.to_string().contains("injected write failure"));
    assert_eq!(simulation.state().step.get(), 0);
    assert_eq!(simulation.ledger().postings(), opening_postings);
    simulation.run().unwrap();
    assert!(!simulation.advance().unwrap());
    assert_eq!(
        journal
            .days
            .iter()
            .map(|d| d.market.state.step.get())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn fresh_directory_journal_persists_ledger_postings_and_terminal_mark() {
    let mut scenario = parse_scenario(SCENARIO).unwrap();
    scenario.burn_in = TradingDays::new(2);
    scenario.observed_days = TradingDays::new(18);
    scenario.funding_rate = FundingRate::new(0.04).unwrap();
    let policy = NeuralPolicy::from_json(MODEL, scenario.trading_days_per_year).unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("market-sim-test-{}-{nonce}", std::process::id()));
    let mut disk = JsonDirectoryJournal::new(&path).unwrap();
    let mut memory = CapturedJournal::default();
    Simulation::new(scenario, &policy, &SeededShocks, &mut memory)
        .unwrap()
        .run()
        .unwrap();
    for day in &memory.0 {
        disk.publish(day).unwrap();
    }
    assert!(disk.publish(&memory.0[0]).is_err());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 21);
    let opening: serde_json::Value =
        serde_json::from_reader(fs::File::open(path.join("0000000000.json")).unwrap()).unwrap();
    assert_eq!(opening["metadata"]["lot_size_micros"], 10_000);
    assert_eq!(opening["metadata"]["fee_account"], 200);
    assert_eq!(opening["metadata"]["funding_account"], 201);
    assert_eq!(opening["metadata"]["call_buyer"], 202);
    assert_eq!(
        opening["metadata"]["tick_value_per_share"]["currency"],
        "USD"
    );
    let cutoff: serde_json::Value =
        serde_json::from_reader(fs::File::open(path.join("0000000002.json")).unwrap()).unwrap();
    assert_eq!(cutoff["burn_in_marks"].as_array().unwrap().len(), 6);
    let record: serde_json::Value =
        serde_json::from_reader(fs::File::open(path.join("0000000020.json")).unwrap()).unwrap();
    assert_eq!(record["step"], 20);
    assert_eq!(record["included_in_metrics"], true);
    assert_eq!(
        record["terminal"]["observed_hedger_pnl"]
            .as_array()
            .unwrap()
            .len(),
        40
    );
    assert!(
        record["postings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|posting| posting["type"] == "call_settlement")
    );
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn observed_pnl_is_rebased_to_marked_equity_after_burn_in() {
    let mut scenario = parse_scenario(SCENARIO).unwrap();
    scenario.funding_rate = FundingRate::new(0.03).unwrap();
    let policy = NeuralPolicy::from_json(MODEL, scenario.trading_days_per_year).unwrap();
    let run = |burn, observed| {
        let mut scenario = scenario;
        scenario.burn_in = TradingDays::new(burn);
        scenario.observed_days = TradingDays::new(observed);
        let mut journal = CapturedJournal::default();
        Simulation::new(scenario, &policy, &SeededShocks, &mut journal)
            .unwrap()
            .run()
            .unwrap();
        journal.0.pop().unwrap().terminal.unwrap()
    };
    let cutoff = run(0, 24);
    let full = run(0, 30);
    let observed = run(24, 6);
    assert!(
        cutoff
            .lifetime_hedger_pnl
            .iter()
            .any(|(_, pnl)| pnl.amount().get().unsigned_abs() > 10)
    );
    assert_eq!(full.lifetime_hedger_pnl, observed.lifetime_hedger_pnl);
    for ((opening, at_close), window) in cutoff
        .lifetime_hedger_pnl
        .iter()
        .zip(full.lifetime_hedger_pnl)
        .zip(observed.observed_hedger_pnl)
    {
        assert_eq!(opening.0, window.0);
        assert_eq!(at_close.0, window.0);
        assert_eq!(at_close.1.checked_sub(opening.1).unwrap(), window.1);
    }
}

#[test]
fn large_negative_rates_produce_finite_call_marks() {
    let mut scenario = parse_scenario(SCENARIO).unwrap();
    scenario.observed_days = TradingDays::new(1);
    scenario.burn_in = TradingDays::new(0);
    scenario.call.maturity = TradingDays::new(1000);
    scenario.trading_days_per_year = TradingDays::new(1);
    scenario.funding_rate = FundingRate::new(-1.0).unwrap();
    let policy = ClassicalPolicy::new(
        ClassicalMethod::BlackScholes,
        scenario.funding_rate,
        scenario.trading_days_per_year,
    )
    .unwrap();
    let mut journal = CapturedJournal::default();
    Simulation::new(scenario, &policy, &SeededShocks, &mut journal)
        .unwrap()
        .run()
        .unwrap();
    let issued_premiums: Vec<_> = journal.0[0]
        .postings
        .iter()
        .filter_map(|p| match p.kind {
            PostingKind::OptionIssue { premium, .. } => Some(premium.amount().get()),
            _ => None,
        })
        .collect();
    assert_eq!(issued_premiums, [0]);
}

#[test]
fn invalid_scenario_and_overflow_report_the_failed_operation() {
    use market_sim::domain::market::TickValuePerShare;
    use market_sim::domain::units::{CashUnits, Money, PriceTicks};

    let scenario = parse_scenario(SCENARIO).unwrap();
    let neural = NeuralPolicy::from_json(MODEL, scenario.trading_days_per_year).unwrap();
    let mut journal = CapturedJournal::default();

    let mut no_days = scenario;
    no_days.observed_days = TradingDays::new(0);
    let Err(error) = Simulation::new(no_days, &neural, &SeededShocks, &mut journal) else {
        panic!("an empty simulation window should fail initialization");
    };
    assert!(matches!(error, SimulationError::Market(_)));
    assert!(error.to_string().starts_with("market day"));

    let mut expensive = scenario;
    let currency = scenario.instrument.tick_value_per_share.get().currency();
    expensive.instrument.tick_value_per_share =
        TickValuePerShare::new(Money::new(currency, CashUnits::new(i64::MAX))).unwrap();
    expensive.initial_close = PriceTicks::new(u64::MAX).unwrap();
    let Err(error) = Simulation::new(expensive, &neural, &SeededShocks, &mut journal) else {
        panic!("an unrepresentable premium should fail initialization");
    };
    assert!(matches!(error, SimulationError::Domain(_)));
    assert_eq!(error.to_string(), "arithmetic overflow");
    assert!(journal.0.is_empty());

    let mut funded = scenario;
    funded.cash_decimal_places =
        market_sim::application::scenario::CashDecimalPlaces::new(9).unwrap();
    funded.funding_rate = FundingRate::new(9.0e9).unwrap();
    let mut simulation = Simulation::new(funded, &neural, &SeededShocks, &mut journal).unwrap();
    let opening_postings = simulation.ledger().postings().to_vec();
    let error = simulation.advance().unwrap_err();
    assert!(matches!(error, SimulationError::Accounting(_)));
    assert_eq!(error.to_string(), "arithmetic overflow");
    assert_eq!(simulation.state().step.get(), 0);
    assert_eq!(simulation.ledger().postings(), opening_postings);
}
