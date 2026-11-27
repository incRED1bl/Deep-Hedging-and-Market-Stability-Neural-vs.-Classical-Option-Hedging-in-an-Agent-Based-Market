"""No-dividend European call; sigma and continuous rate are annual fractions.

Source: Black & Scholes (1973), https://doi.org/10.1086/260062.
AI assistance: ChatGPT; checked against analytic limits and finite differences.
"""

from dataclasses import dataclass
from math import erfc, exp, isfinite, log, log1p, pi, sqrt

from .accounting import RATE_SCALE, _rate_units


@dataclass(frozen=True)
class CallMetrics:
    price: float
    delta: float
    gamma: float | None
    vega: float


def funding_to_continuous_rate(annual_simple_rate: float, days_per_year: int = 252) -> float:
    """Match (1 + simple_rate / D)**days before cash rounding in the ledger."""
    if not isfinite(annual_simple_rate) or type(days_per_year) is not int or days_per_year <= 0:
        raise ValueError("finite funding rate and positive day count required")
    annual_simple_rate = _rate_units(annual_simple_rate) / RATE_SCALE
    daily_rate = annual_simple_rate / days_per_year
    if daily_rate <= -1:
        raise ValueError("daily funding factor must be positive")
    return days_per_year * log1p(daily_rate)


def european_call(
    spot: float,
    strike: float,
    years: float,
    volatility: float,
    rate: float = 0.0,
) -> CallMetrics:
    """Price and per-share delta/gamma/vega; vega is per unit sigma, not per 1%.

    At a deterministic payoff kink delta=0.5 is a convention and gamma=None:
    a classical second derivative does not exist there. Vega at sigma=0 is
    the right derivative; at expiry vega=0. No volatility floor is introduced.
    """
    if not all(isfinite(value) for value in (spot, strike, years, volatility, rate)):
        raise ValueError("pricing inputs must be finite")
    if spot <= 0 or strike <= 0 or years < 0 or volatility < 0:
        raise ValueError("positive prices, nonnegative time and volatility required")
    if years == 0:
        gap = spot - strike
        return CallMetrics(max(gap, 0.0), _step_delta(gap), None if gap == 0 else 0.0, 0.0)

    try:
        discounted_strike = strike * exp(-rate * years)
    except OverflowError as error:
        raise ValueError("discounted strike exceeds float range") from error
    if not isfinite(discounted_strike):
        raise ValueError("discounted strike exceeds float range")
    total_vol = volatility * sqrt(years)
    if not isfinite(total_vol):
        raise ValueError("total volatility exceeds float range")
    if total_vol == 0:
        gap = spot - discounted_strike
        vega = spot * sqrt(years / (2 * pi)) if gap == 0 else 0.0
        return CallMetrics(max(gap, 0.0), _step_delta(gap), None if gap == 0 else 0.0, vega)

    d1 = (log(spot) - log(strike) + rate * years) / total_vol + total_vol / 2
    d2 = d1 - total_vol
    delta = _cdf(d1)
    density = exp(-0.5 * d1 * d1) / sqrt(2 * pi)
    intrinsic = max(spot - discounted_strike, 0.0)
    # Evaluate the out-of-the-money leg, then use parity for an ITM call.
    if spot > discounted_strike:
        price = intrinsic + discounted_strike * _cdf(-d2) - spot * _cdf(-d1)
    else:
        price = spot * delta - discounted_strike * _cdf(d2)
    gamma = density / spot / total_vol
    vega = spot * density * sqrt(years)
    if not all(isfinite(value) for value in (price, gamma, vega)):
        raise ValueError("price or Greek exceeds float range")
    return CallMetrics(min(spot, max(intrinsic, price)), delta, gamma, vega)


def _cdf(value: float) -> float:
    return 0.5 * erfc(-value / sqrt(2))


def _step_delta(gap: float) -> float:
    return 1.0 if gap > 0 else 0.0 if gap < 0 else 0.5
