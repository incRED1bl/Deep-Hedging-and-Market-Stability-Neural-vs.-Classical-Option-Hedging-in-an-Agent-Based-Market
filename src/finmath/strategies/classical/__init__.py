"""Daily short-call hedging references for issue #10.

Leland convention: Nguyen & Pergamenshchikov, arXiv:1505.02546v1, (5)--(7).
Band projection: Imaki et al., arXiv:2103.01775v1, (12)--(13).
AI assistance: ChatGPT; checked with analytic limits and shared target fixtures.
"""

from dataclasses import dataclass
from math import exp, fsum, isfinite, log, pi, sqrt

from finmath.finance import european_call, funding_to_continuous_rate
from finmath.finance.accounting import RATE_SCALE, _rate_units

BAND_WIDTHS = (0.0, 0.01, 0.025, 0.05, 0.10, 0.20)


@dataclass(frozen=True)
class CallObservation:
    """One short European call, matching the project's seller cohort."""

    moneyness: float
    remaining_days: int
    size_shares: int

    def __post_init__(self) -> None:
        if not isfinite(self.moneyness) or self.moneyness <= 0:
            raise ValueError("positive finite moneyness required")
        if type(self.remaining_days) is not int or self.remaining_days < 0:
            raise ValueError("nonnegative integer remaining days required")
        if type(self.size_shares) is not int or self.size_shares <= 0:
            raise ValueError("positive integer short-call exposure required")


@dataclass(frozen=True)
class Observation:
    shares_held: float
    annualized_volatility: float
    commission: float
    call: CallObservation | None

    def __post_init__(self) -> None:
        if not isfinite(self.shares_held):
            raise ValueError("finite actual holdings required")
        if not isfinite(self.annualized_volatility) or self.annualized_volatility < 0:
            raise ValueError("nonnegative finite annual volatility required")
        _rate_units(self.commission, nonnegative=True)


def leland_volatility(volatility: float, one_way_commission: float, years_per_step: float) -> float:
    """Upper adjustment for a call writer: sigma_L²=sigma²+c*sigma*sqrt(8/(pi*dt))."""
    if not isfinite(volatility) or volatility < 0:
        raise ValueError("nonnegative finite volatility required")
    if not isfinite(years_per_step) or years_per_step <= 0:
        raise ValueError("positive finite revision interval required")
    c = _rate_units(one_way_commission, nonnegative=True) / RATE_SCALE
    if volatility == 0 or c == 0:
        return volatility
    adjusted = sqrt(volatility) * sqrt(volatility + c * sqrt(8 / pi) / sqrt(years_per_step))
    if not isfinite(adjusted):
        raise ValueError("adjusted volatility exceeds float range")
    return adjusted


@dataclass(frozen=True)
class ClassicalPolicy:
    method: str
    days_per_year: int = 252
    annual_funding_rate: float = 0.0
    band_width: float = 0.0

    def __post_init__(self) -> None:
        if self.method not in {"bs", "leland", "band"}:
            raise ValueError("unknown classical policy")
        funding_to_continuous_rate(self.annual_funding_rate, self.days_per_year)
        if not isfinite(self.band_width) or not 0 <= self.band_width <= 1:
            raise ValueError("band half-width must be in [0,1]")
        if self.method != "band" and self.band_width != 0:
            raise ValueError("band_width is only valid for band")

    def target(self, observation: Observation) -> float:
        call = observation.call
        if call is None or call.remaining_days == 0:
            return 0.0
        sigma = observation.annualized_volatility
        if self.method == "leland":
            sigma = leland_volatility(sigma, observation.commission, 1 / self.days_per_year)
        delta = european_call(
            call.moneyness,
            1.0,
            call.remaining_days / self.days_per_year,
            sigma,
            funding_to_continuous_rate(self.annual_funding_rate, self.days_per_year),
        ).delta
        center = call.size_shares * delta
        if self.method == "band":
            width = call.size_shares * self.band_width
            lower = max(0.0, center - width)
            upper = min(float(call.size_shares), center + width)
            return min(upper, max(lower, observation.shares_held))
        return center

    def targets(self, observations: list[Observation]) -> list[float]:
        return [self.target(observation) for observation in observations]


def select_band_width(
    validation_pnls: dict[float, list[float]], *, split: str, risk_aversion: float = 50.0
) -> float:
    """Select per commission using matched validation episodes' normalized net P&L.

    Minimize log(mean(exp(-lambda*z)))/lambda; ties within 1e-8 prefer smaller w.
    The caller must enforce split provenance and supply the same episodes for every width.
    """
    if split != "validation":
        raise ValueError("policy selection requires validation results")
    if set(validation_pnls) != set(BAND_WIDTHS):
        raise ValueError("all predefined band widths must be scored")
    if not isfinite(risk_aversion) or risk_aversion <= 0:
        raise ValueError("positive finite risk aversion required")
    counts = {len(pnls) for pnls in validation_pnls.values()}
    if len(counts) != 1 or 0 in counts:
        raise ValueError("matching nonempty validation episodes required")
    losses = {}
    for width, pnls in validation_pnls.items():
        if not all(isfinite(pnl) for pnl in pnls):
            raise ValueError("finite normalized net P&L required")
        worst = min(pnls)
        # Shift before exponentiation: the exponents are nonpositive, even for large losses.
        loss = (
            -worst
            + log(fsum(exp(-risk_aversion * (z - worst)) for z in pnls) / len(pnls)) / risk_aversion
        )
        if not isfinite(loss):
            raise ValueError("validation risk exceeds float range")
        losses[width] = loss
    best = min(losses.values())
    return min(width for width, loss in losses.items() if loss <= best + 1e-8)
