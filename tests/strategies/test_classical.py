"""Financial controls independent of the target implementation; no market/test data."""

import json
from dataclasses import replace
from math import inf, nan, sqrt
from pathlib import Path

import pytest

from finmath.finance import european_call, funding_to_continuous_rate
from finmath.strategies.classical import (
    BAND_WIDTHS,
    CallObservation,
    ClassicalPolicy,
    Observation,
    leland_volatility,
    select_band_width,
)

ROOT = Path(__file__).parents[2]
FIXTURE = json.loads((ROOT / "tests/fixtures/classical-strategies.json").read_text())
OBS = Observation(0.0, 0.2, 0.001, CallObservation(1.0, 20, 100))


@pytest.mark.parametrize("case", FIXTURE["cases"])
def test_shared_target_controls(case):
    obs = Observation(
        case["shares_held"],
        case["annualized_volatility"],
        case["commission"],
        CallObservation(case["moneyness"], case["remaining_days"], case["size_shares"]),
    )
    policy = ClassicalPolicy(
        case["method"],
        band_width=case["band_width"],
        days_per_year=case["days_per_year"],
        annual_funding_rate=case["annual_funding_rate"],
    )
    assert policy.targets([obs]) == pytest.approx([case["expected_target"]], abs=1e-12, rel=0)


def test_leland_one_way_factor_and_zero_cost_limit():
    case = FIXTURE["leland_sigma_control"]
    actual = leland_volatility(case["sigma"], case["one_way_commission"], case["years_per_step"])
    assert actual == pytest.approx(case["expected"], abs=1e-15)
    # Expected one-day absolute delta turnover balances the extra gamma term.
    dt, gamma, spot = 1 / 252, 0.03, 100.0
    extra_variance = actual**2 - 0.2**2
    assert 0.5 * extra_variance * spot**2 * gamma * dt == pytest.approx(
        0.001 * spot**2 * gamma * 0.2 * sqrt(2 * dt / 3.141592653589793)
    )
    assert leland_volatility(0.2, 0, dt) == 0.2
    assert leland_volatility(0, 0.001, dt) == 0
    assert leland_volatility(0.2, 1e-12, dt) == 0.2  # ledger quantizes c to zero
    zero_cost = replace(OBS, commission=0)
    assert ClassicalPolicy("leland").target(zero_cost) == ClassicalPolicy("bs").target(zero_cost)
    assert leland_volatility(0.2, 0.001, dt / 2) > actual
    assert leland_volatility(0.2, 0.002, dt) > actual


def test_band_projects_actual_holdings_to_nearest_edge():
    center = ClassicalPolicy("bs").target(OBS)
    policy = ClassicalPolicy("band", band_width=0.05)
    lower, upper = center - 5, center + 5
    for held in (lower, center, 50, upper):
        assert policy.target(replace(OBS, shares_held=held)) == held
    assert policy.target(OBS) == lower
    assert policy.target(replace(OBS, shares_held=100)) == upper
    # A one-share partial fill does not establish the requested 46-share target.
    assert policy.target(replace(OBS, shares_held=1)) == lower
    assert ClassicalPolicy("band").target(OBS) == center
    for held in (-10, 120):
        wide = ClassicalPolicy("band", band_width=1)
        assert wide.target(replace(OBS, shares_held=held)) == min(100, max(0, held))


@pytest.mark.parametrize("method", ["bs", "leland", "band"])
def test_missing_or_expired_call_is_flat_and_batch_order_is_preserved(method):
    policy = ClassicalPolicy(method)
    expired = replace(OBS, call=CallObservation(1, 0, 100), shares_held=50)
    absent = replace(OBS, call=None, shares_held=50)
    assert policy.targets([OBS, absent, expired]) == [policy.target(OBS), 0, 0]
    assert policy.targets([]) == []


def test_zero_sigma_rate_and_exposure_conventions():
    deterministic = replace(OBS, annualized_volatility=0)
    for method in ("bs", "leland"):
        assert ClassicalPolicy(method).target(deterministic) == 50
    policy = ClassicalPolicy("bs", annual_funding_rate=0.1)
    expected = european_call(1, 1, 20 / 252, 0.2, funding_to_continuous_rate(0.1)).delta
    assert policy.target(OBS) == 100 * expected
    scaled = replace(OBS, call=CallObservation(1, 20, 200))
    assert policy.target(scaled) == 2 * policy.target(OBS)


def test_selection_uses_entropic_risk_not_mean_pnl_and_is_stable():
    pnls = {width: [-0.01, -0.01] for width in BAND_WIDTHS}
    pnls[0.05] = [0.1, -0.1]  # higher mean, worse exponential downside risk
    pnls[0.1] = [0, 0]
    assert select_band_width(pnls, split="validation") == 0.1
    large = {width: [-5000, -5000] for width in BAND_WIDTHS}
    large[0.2] = [-4999, -5000]
    assert select_band_width(large, split="validation") == 0.2
    tied = {width: [0, 0] for width in BAND_WIDTHS}
    tied[0.2] = [5e-9, 5e-9]
    assert select_band_width(tied, split="validation") == 0


def test_configuration_loads_and_matches_predefined_selection():
    config = json.loads((ROOT / "configs/classical-strategies.json").read_text())
    assert config["status"] == "reference-defaults-not-validation-selected"
    assert tuple(config["validation"]["band_width_grid"]) == BAND_WIDTHS
    assert config["validation"]["risk_aversion"] == 50
    assert config["validation"]["tie_tolerance"] == 1e-8
    for name, params in config["policies"].items():
        assert ClassicalPolicy(**params).method == name


@pytest.mark.parametrize("split", ["train", "test", "final-test"])
def test_selection_rejects_other_splits(split):
    with pytest.raises(ValueError, match="validation"):
        select_band_width({w: [0] for w in BAND_WIDTHS}, split=split)


@pytest.mark.parametrize("change", ["missing", "empty", "unmatched", "nan"])
def test_selection_rejects_incomplete_or_invalid_results(change):
    pnls = {w: [0, 0] for w in BAND_WIDTHS}
    if change == "missing":
        pnls.pop(0)
    elif change == "empty":
        pnls = {w: [] for w in BAND_WIDTHS}
    elif change == "unmatched":
        pnls[0] = [0]
    else:
        pnls[0] = [nan, 0]
    with pytest.raises(ValueError):
        select_band_width(pnls, split="validation")


@pytest.mark.parametrize("risk_aversion", [0, -1, nan, inf])
def test_selection_rejects_invalid_risk_aversion(risk_aversion):
    with pytest.raises(ValueError):
        select_band_width(
            {w: [0] for w in BAND_WIDTHS}, split="validation", risk_aversion=risk_aversion
        )


@pytest.mark.parametrize(
    "factory",
    [
        lambda: CallObservation(0, 20, 100),
        lambda: CallObservation(nan, 20, 100),
        lambda: CallObservation(1, -1, 100),
        lambda: CallObservation(1, 20.5, 100),
        lambda: CallObservation(1, 20, 0),
        lambda: replace(OBS, shares_held=inf),
        lambda: replace(OBS, annualized_volatility=-0.1),
        lambda: replace(OBS, commission=-0.1),
        lambda: ClassicalPolicy("unknown"),
        lambda: ClassicalPolicy("band", band_width=nan),
        lambda: ClassicalPolicy("band", band_width=1.1),
        lambda: ClassicalPolicy("bs", band_width=0.1),
        lambda: ClassicalPolicy("bs", days_per_year=0),
        lambda: ClassicalPolicy("bs", annual_funding_rate=-252),
        lambda: leland_volatility(-0.1, 0.001, 1 / 252),
        lambda: leland_volatility(0.2, 0.001, 0),
    ],
)
def test_invalid_financial_inputs_fail(factory):
    with pytest.raises(ValueError):
        factory()
