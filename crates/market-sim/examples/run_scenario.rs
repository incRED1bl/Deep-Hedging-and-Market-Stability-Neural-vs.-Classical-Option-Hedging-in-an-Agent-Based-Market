use std::env;
use std::error::Error;
use std::fs;
use std::io;

use market_sim::application::simulation::Simulation;
use market_sim::domain::classical::{BandWidth, ClassicalMethod, ClassicalPolicy};
use market_sim::domain::hedging::HedgingPolicy;
use market_sim::infrastructure::config::parse_scenario;
use market_sim::infrastructure::journal::JsonDirectoryJournal;
use market_sim::infrastructure::neural_policy::NeuralPolicy;
use market_sim::infrastructure::shocks::SeededShocks;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [config, method, journal, model @ ..] = args.as_slice() else {
        return Err(usage().into());
    };
    let scenario = parse_scenario(&fs::read_to_string(config)?)?;
    let classical = |method| {
        ClassicalPolicy::new(
            method,
            scenario.funding_rate,
            scenario.trading_days_per_year,
        )
    };
    let policy: Box<dyn HedgingPolicy> = match (method.as_str(), model) {
        ("bs", []) => Box::new(classical(ClassicalMethod::BlackScholes)?),
        ("leland", []) => Box::new(classical(ClassicalMethod::Leland)?),
        ("band", []) => Box::new(classical(ClassicalMethod::Band {
            width: BandWidth::new(0.05)?,
        })?),
        ("neural", [model]) => Box::new(NeuralPolicy::from_json(
            &fs::read_to_string(model)?,
            scenario.trading_days_per_year,
        )?),
        _ => return Err(usage().into()),
    };

    let mut journal = JsonDirectoryJournal::new(journal)?;
    let mut simulation = Simulation::new(scenario, &*policy, &SeededShocks, &mut journal)?;
    simulation.run()?;
    let state = simulation.state();
    println!(
        "Completed {} trading days. Final close {} ticks.",
        state.step.get(),
        state.close.get()
    );
    Ok(())
}

fn usage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "usage: run_scenario CONFIG.json {bs|leland|band|neural} JOURNAL_DIR [MODEL.json]",
    )
}
