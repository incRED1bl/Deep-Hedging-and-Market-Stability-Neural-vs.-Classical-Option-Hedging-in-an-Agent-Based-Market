use std::cell::RefCell;

use serde::Deserialize;

use crate::domain::classical::hedge_exposure;
use crate::domain::hedging::{HedgingPolicy, Observation, PolicyError, TargetPosition};
use crate::domain::units::{Shares, TradingDays};

const FEATURES: usize = 6;

#[derive(Debug)]
pub enum ModelError {
    Json(serde_json::Error),
    InvalidModel,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::InvalidModel => {
                formatter.write_str("invalid model version, hidden layer or day count")
            }
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HiddenUnit {
    weights: [f64; FEATURES],
    bias: f64,
    output_weight: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    schema_version: u32,
    hidden: Vec<HiddenUnit>,
    output_bias: f64,
}

#[derive(Clone, Debug)]
pub struct NeuralPolicy {
    model: Model,
    days_per_year: TradingDays,
    rows: RefCell<Vec<InferenceRow>>,
}

#[derive(Clone, Debug)]
struct InferenceRow {
    index: usize,
    features: [f64; FEATURES],
    exposure: f64,
    logit: f64,
}

impl NeuralPolicy {
    pub fn from_json(input: &str, days_per_year: TradingDays) -> Result<Self, ModelError> {
        let model: Model = serde_json::from_str(input).map_err(ModelError::Json)?;
        if model.schema_version != 1 || model.hidden.is_empty() || days_per_year.get() == 0 {
            return Err(ModelError::InvalidModel);
        }
        Ok(Self {
            model,
            days_per_year,
            rows: RefCell::new(Vec::new()),
        })
    }
}

impl HedgingPolicy for NeuralPolicy {
    fn targets(
        &self,
        observations: &[Observation],
        targets: &mut [TargetPosition],
    ) -> Result<(), PolicyError> {
        if observations.len() != targets.len() {
            return Err(PolicyError::BatchLengthMismatch);
        }
        let mut batch = self.rows.borrow_mut();
        batch.clear();
        for (index, (observation, target)) in
            observations.iter().zip(targets.iter_mut()).enumerate()
        {
            let Some(call) = observation.call else {
                target.target_shares = Shares::new(0.0).expect("zero shares");
                continue;
            };
            let exposure = hedge_exposure(call.underlying_exposure);
            batch.push(InferenceRow {
                index,
                features: [
                    call.moneyness.get().ln(),
                    f64::from(call.time_to_expiry.get()) / f64::from(self.days_per_year.get()),
                    observation.annualized_volatility.get(),
                    observation.shares_held.get() / exposure.abs(),
                    exposure.signum(),
                    observation.commission.get(),
                ],
                exposure,
                logit: self.model.output_bias,
            });
        }
        for unit in &self.model.hidden {
            for row in batch.iter_mut() {
                let activation = unit
                    .weights
                    .iter()
                    .zip(row.features)
                    .fold(unit.bias, |sum, (weight, feature)| sum + weight * feature);
                row.logit += unit.output_weight * activation.tanh();
            }
        }
        for row in batch.iter() {
            if !row.logit.is_finite() {
                return Err(PolicyError::EvaluationFailed);
            }
            let shares = row.exposure * (0.5 + 0.5 * (row.logit / 2.0).tanh());
            targets[row.index].target_shares =
                Shares::new(shares).expect("sigmoid exposure is finite");
        }
        Ok(())
    }
}
