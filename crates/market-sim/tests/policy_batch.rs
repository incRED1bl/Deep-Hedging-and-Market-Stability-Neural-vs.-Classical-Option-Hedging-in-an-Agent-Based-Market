use market_sim::domain::classical::{BandWidth, ClassicalMethod, ClassicalPolicy};
use market_sim::domain::hedging::{
    CallExposure, CallObservation, HedgingPolicy, Moneyness, Observation, PolicyError,
    TargetPosition,
};
use market_sim::domain::units::{
    AnnualizedVolatility, ContractSize, FundingRate, MarketStep, OneWayCommissionRate, Shares,
    TradingDays,
};
use market_sim::infrastructure::neural_policy::{ModelError, NeuralPolicy};

const MODEL: &str = include_str!("fixtures/prototype-model.json");
const YEAR: TradingDays = TradingDays::new(252);

fn observation(
    moneyness: f64,
    remaining: u32,
    volatility: f64,
    held: f64,
    exposure: i32,
    fee: f64,
) -> Observation {
    let size = ContractSize::new(exposure.unsigned_abs()).unwrap();
    Observation {
        as_of: MarketStep::new(2),
        shares_held: Shares::new(held).unwrap(),
        annualized_volatility: AnnualizedVolatility::new(volatility).unwrap(),
        commission: OneWayCommissionRate::new(fee).unwrap(),
        call: Some(CallObservation {
            moneyness: Moneyness::new(moneyness).unwrap(),
            time_to_expiry: TradingDays::new(remaining),
            underlying_exposure: if exposure > 0 {
                CallExposure::Short(size)
            } else {
                CallExposure::Long(size)
            },
        }),
    }
}

fn empty_target() -> TargetPosition {
    TargetPosition {
        target_shares: Shares::new(0.0).unwrap(),
    }
}

#[test]
fn layer_major_batch_matches_independently_calculated_scalar_outputs() {
    let policy = NeuralPolicy::from_json(MODEL, YEAR).unwrap();
    let first = observation(1.04, 16, 0.22, 0.33, 1, 0.001);
    let second = observation(0.93, 4, 0.30, -0.40, -2, 0.003);
    let third = observation(1.0, 20, 0.19, 0.0, 1, 0.0);
    let input = [
        first,
        Observation {
            call: None,
            ..first
        },
        second,
        third,
    ];
    let mut output = [empty_target(); 4];
    policy.targets(&input, &mut output).unwrap();

    // Scalar reference uses a per-row dot product and logistic exp, not batched tanh.
    let expected = [
        0.722_429_277_812_440_5,
        0.0,
        -0.286_230_821_763_369_3,
        0.448_014_077_719_748_55,
    ];
    for (target, expected) in output.iter().zip(expected) {
        assert!((target.target_shares.get() - expected).abs() < 1e-14);
    }
    let mut single = [empty_target()];
    policy.targets(&[third], &mut single).unwrap();
    assert_eq!(single[0], output[3]);
    assert_eq!(
        policy.targets(&input, &mut single),
        Err(PolicyError::BatchLengthMismatch)
    );
}

#[test]
fn invalid_model_shape_and_nonfinite_inference_are_rejected() {
    let invalid =
        NeuralPolicy::from_json(r#"{"schema_version":1,"hidden":[],"output_bias":0}"#, YEAR)
            .unwrap_err();
    assert!(matches!(invalid, ModelError::InvalidModel));
    assert!(invalid.to_string().contains("hidden layer"));
    let malformed =
        NeuralPolicy::from_json(&MODEL.replace("0.0, 0.0, -4.0", "0.0, 0.0"), YEAR).unwrap_err();
    assert!(matches!(malformed, ModelError::Json(_)));
    assert!(malformed.to_string().contains("length"));
    assert!(NeuralPolicy::from_json(MODEL, TradingDays::new(0)).is_err());
    assert!(NeuralPolicy::from_json(&MODEL.replace("version\": 1", "version\": 2"), YEAR).is_err());

    let weights = r#"{"schema_version":1,"hidden":[{"weights":[0,0,1e308,-1e308,0,0],"bias":0,"output_weight":1}],"output_bias":0}"#;
    let policy = NeuralPolicy::from_json(weights, YEAR).unwrap();
    let input = [observation(1.0, 2, f64::MAX, f64::MAX, 1, 0.0)];
    assert_eq!(
        policy.targets(&input, &mut [empty_target()]),
        Err(PolicyError::EvaluationFailed)
    );
}

#[test]
fn classical_policies_exercise_limits_and_position_sign() {
    let rate = FundingRate::new(0.0).unwrap();
    assert!(BandWidth::new(-0.1).is_err());
    assert!(BandWidth::new(f64::NAN).is_err());
    assert!(
        ClassicalPolicy::new(ClassicalMethod::BlackScholes, rate, TradingDays::new(0)).is_err()
    );
    let policy = ClassicalPolicy::new(ClassicalMethod::BlackScholes, rate, YEAR).unwrap();
    let obs = observation(1.0, 0, 0.22, 0.1, -2, 0.0);
    assert_eq!(
        policy.targets(&[obs], &mut []),
        Err(PolicyError::BatchLengthMismatch)
    );
    let mut target = [empty_target()];
    policy.targets(&[obs], &mut target).unwrap();
    assert_eq!(target[0].target_shares.get(), -1.0);
}
