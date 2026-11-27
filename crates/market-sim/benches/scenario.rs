use std::error::Error;
use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use market_sim::application::simulation::Simulation;
use market_sim::domain::classical::{BandWidth, ClassicalMethod, ClassicalPolicy};
use market_sim::domain::hedging::HedgingPolicy;
use market_sim::infrastructure::config::parse_scenario;
use market_sim::infrastructure::journal::JsonDirectoryJournal;
use market_sim::infrastructure::neural_policy::NeuralPolicy;
use market_sim::infrastructure::shocks::SeededShocks;

fn main() -> Result<(), Box<dyn Error>> {
    let scenario = parse_scenario(include_str!("../tests/fixtures/integrated-scenario.json"))?;
    let classical = |method| {
        ClassicalPolicy::new(
            method,
            scenario.funding_rate,
            scenario.trading_days_per_year,
        )
    };
    let policies: [(&str, &dyn HedgingPolicy); 4] = [
        ("Black–Scholes", &classical(ClassicalMethod::BlackScholes)?),
        ("Leland", &classical(ClassicalMethod::Leland)?),
        (
            "Band",
            &classical(ClassicalMethod::Band {
                width: BandWidth::new(0.05)?,
            })?,
        ),
        (
            "Neural",
            &NeuralPolicy::from_json(
                include_str!("../tests/fixtures/prototype-model.json"),
                scenario.trading_days_per_year,
            )?,
        ),
    ];

    for (index, (name, policy)) in policies.iter().enumerate() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory =
            std::env::temp_dir().join(format!("market-sim-{}-{nonce}-{index}", std::process::id()));
        let mut journal = JsonDirectoryJournal::new(&directory)?;
        let started = Instant::now();
        let result = (|| {
            let mut simulation = Simulation::new(scenario, *policy, &SeededShocks, &mut journal)?;
            simulation.run()?;
            Ok::<_, market_sim::application::simulation::SimulationError>(
                simulation.state().step.get(),
            )
        })();
        let elapsed = started.elapsed().as_secs_f64();
        fs::remove_dir_all(directory)?;
        let days = result?;
        println!(
            "{name:>15} {days} trading days in {elapsed:.3} s ({:.0} days/s)",
            f64::from(days) / elapsed
        );
    }
    Ok(())
}
