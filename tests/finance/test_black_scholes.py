"""Analytic limits and numerical derivatives, independent of simulator code.

AI assistance: ChatGPT; inputs and tolerances are explicit below.
"""

import json
import math
from pathlib import Path

import pytest

from finmath.finance import european_call, funding_to_continuous_rate


def test_standard_call_value_and_greeks() -> None:
    value = european_call(100, 100, 1, 0.2, 0.05)
    assert value.price == pytest.approx(10.450583572185565, abs=1e-12)
    assert value.delta == pytest.approx(0.6368306511756191, abs=1e-12)
    assert value.gamma == pytest.approx(0.018762017345846895, abs=1e-14)
    assert value.vega == pytest.approx(37.52403469169379, abs=1e-12)


@pytest.mark.parametrize("spot", [60, 95, 100, 105, 160])
@pytest.mark.parametrize("rate", [-0.03, 0, 0.05])
def test_greeks_against_price_finite_differences(spot: float, rate: float) -> None:
    strike, years, sigma, h = 100, 20 / 252, 0.3, 0.01
    value = european_call(spot, strike, years, sigma, rate)
    lo = european_call(spot - h, strike, years, sigma, rate).price
    hi = european_call(spot + h, strike, years, sigma, rate).price
    assert value.delta == pytest.approx((hi - lo) / (2 * h), abs=1e-7)
    assert value.gamma == pytest.approx((hi - 2 * value.price + lo) / h**2, abs=2e-8)
    dv = 1e-5
    numeric_vega = (
        european_call(spot, strike, years, sigma + dv, rate).price
        - european_call(spot, strike, years, sigma - dv, rate).price
    ) / (2 * dv)
    assert value.vega == pytest.approx(numeric_vega, abs=1e-7)
    assert 0 <= value.delta <= 1
    assert value.gamma is not None and value.gamma >= 0
    assert max(spot - strike * math.exp(-rate * years), 0) <= value.price <= spot


@pytest.mark.parametrize("spot,delta", [(90, 0), (100, 0.5), (110, 1)])
def test_expiry_intrinsic_and_delta_convention(spot: float, delta: float) -> None:
    value = european_call(spot, 100, 0, 0.2)
    assert value.price == max(spot - 100, 0)
    assert value.delta == delta
    assert value.gamma == (None if spot == 100 else 0)
    assert value.vega == 0


def test_zero_and_small_volatility_without_an_artificial_floor() -> None:
    zero = european_call(100, 100, 1, 0)
    assert zero.price == 0 and zero.delta == 0.5 and zero.gamma is None
    assert zero.vega == pytest.approx(100 / math.sqrt(2 * math.pi))
    tiny = european_call(100, 100, 1, 1e-10)
    assert tiny.price == pytest.approx(100 * 1e-10 / math.sqrt(2 * math.pi), abs=2e-14)
    assert math.isfinite(tiny.gamma)
    deterministic = european_call(120, 100, 1, 0, 0.25)
    assert deterministic.price == pytest.approx(120 - 100 * math.exp(-0.25))
    assert deterministic.delta == 1 and deterministic.gamma == deterministic.vega == 0


def test_price_discount_matches_the_ledgers_daily_funding_factor() -> None:
    rate = funding_to_continuous_rate(0.25, 1)
    assert european_call(120, 100, 1, 0, rate).price == pytest.approx(40)
    r = funding_to_continuous_rate(0.05, 252)
    assert math.exp(r * 20 / 252) == pytest.approx((1 + 0.05 / 252) ** 20)
    assert funding_to_continuous_rate(0) == 0
    assert funding_to_continuous_rate(1e-10) == 0  # The ledger quantizes rates to 1e-9.
    with pytest.raises(ValueError):
        funding_to_continuous_rate(-1, 1)


@pytest.mark.parametrize(
    "args",
    [
        (0, 100, 1, 0.2),
        (100, -1, 1, 0.2),
        (100, 100, -1, 0.2),
        (100, 100, 1, -0.2),
        (float("nan"), 100, 1, 0.2),
        (100, 100, 1, float("inf")),
        (100, 100, 1, 0.2, float("nan")),
    ],
)
def test_invalid_pricing_inputs_are_rejected(args: tuple[float, ...]) -> None:
    with pytest.raises(ValueError):
        european_call(*args)


def test_shared_pricing_fixtures() -> None:
    path = Path(__file__).parent.parent / "fixtures/finance-reference.json"
    for case in json.loads(path.read_text())["pricing_cases"]:
        metrics = european_call(
            *(case[key] for key in ("spot", "strike", "years", "volatility", "rate"))
        )
        for key in ("price", "delta", "gamma", "vega"):
            assert getattr(metrics, key) == pytest.approx(case[key], abs=1e-12)
