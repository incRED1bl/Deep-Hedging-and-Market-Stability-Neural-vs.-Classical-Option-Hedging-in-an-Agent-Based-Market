"""Run-level market and hedging metrics for issue #13.

Definitions follow research protocol #1 (docs/protocol.md, section 4):
- Realized volatility V uses M observed log returns with ddof=1 and sqrt(D) annualization.
- Market turnover U counts each bilateral fill once, in shares/day.
- Contract hedging errors aggregate only complete contracts inside [burn_in, burn_in + observed],
  normalized by size_shares * issue_spot, with epsilon = -net_pnl / notional (shortfall > 0).
- Burn-in days are excluded from all observed metrics.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from math import ceil, fsum, isfinite, log, sqrt
from typing import Any

METRICS_SCHEMA_VERSION = 1
ALLOWED_POLICIES = frozenset({"bs", "leland", "band", "dl"})


def _check_finite(value: float, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float) or not isfinite(value):
        raise ValueError(f"{name} must be a finite real number")
    return float(value)


def _check_nonnegative(value: float, name: str) -> float:
    val = _check_finite(value, name)
    if val < 0.0:
        raise ValueError(f"{name} must be nonnegative")
    return val


def _check_positive(value: float, name: str) -> float:
    val = _check_finite(value, name)
    if val <= 0.0:
        raise ValueError(f"{name} must be strictly positive")
    return val


def _check_int(value: int, name: str, *, minimum: int = 0) -> int:
    if type(value) is not int or value < minimum:
        raise ValueError(f"{name} must be an integer >= {minimum}")
    return value


@dataclass(frozen=True)
class DailyMarketRecord:
    """Committed end-of-day state for a single market step."""

    step: int
    close_price: float
    bilateral_executed_shares: float
    hedger_executed_shares: float = 0.0
    hedger_commission_paid: float = 0.0
    hedger_total_equity: float = 0.0
    hedger_min_equity: float | None = None
    hedger_requested_shares: float = 0.0
    hedger_unfilled_shares: float = 0.0
    limit_hits: int = 0
    open_calls_shares: float = 0.0
    dollar_volume: float | None = None
    positive_cost_fills: int = 0
    zero_booked_fee_fills: int = 0

    def __post_init__(self) -> None:
        _check_int(self.step, "step", minimum=0)
        _check_positive(self.close_price, "close_price")
        _check_nonnegative(self.bilateral_executed_shares, "bilateral_executed_shares")
        _check_nonnegative(self.hedger_executed_shares, "hedger_executed_shares")
        _check_nonnegative(self.hedger_commission_paid, "hedger_commission_paid")
        _check_finite(self.hedger_total_equity, "hedger_total_equity")
        if self.hedger_min_equity is not None:
            _check_finite(self.hedger_min_equity, "hedger_min_equity")
        req = _check_nonnegative(self.hedger_requested_shares, "hedger_requested_shares")
        unfilled = _check_nonnegative(self.hedger_unfilled_shares, "hedger_unfilled_shares")
        if unfilled > req + 1e-12:
            raise ValueError("hedger_unfilled_shares cannot exceed hedger_requested_shares")
        _check_int(self.limit_hits, "limit_hits", minimum=0)
        _check_nonnegative(self.open_calls_shares, "open_calls_shares")
        if self.dollar_volume is not None:
            _check_nonnegative(self.dollar_volume, "dollar_volume")
        pos_fills = _check_int(self.positive_cost_fills, "positive_cost_fills", minimum=0)
        zero_fills = _check_int(self.zero_booked_fee_fills, "zero_booked_fee_fills", minimum=0)
        if zero_fills > pos_fills:
            raise ValueError("zero_booked_fee_fills cannot exceed positive_cost_fills")


@dataclass(frozen=True)
class ContractOutcome:
    """Self-financing hedge subledger result for one short call contract."""

    issue_step: int
    expiry_step: int
    size_shares: float
    issue_spot: float
    net_pnl: float
    settled: bool = True

    def __post_init__(self) -> None:
        issue = _check_int(self.issue_step, "issue_step", minimum=0)
        expiry = _check_int(self.expiry_step, "expiry_step", minimum=1)
        if expiry <= issue:
            raise ValueError("expiry_step must be strictly greater than issue_step")
        _check_positive(self.size_shares, "size_shares")
        _check_positive(self.issue_spot, "issue_spot")
        _check_finite(self.net_pnl, "net_pnl")
        if type(self.settled) is not bool:
            raise ValueError("settled must be a boolean")


@dataclass(frozen=True)
class ContractErrorSummary:
    """Within-run aggregation of normalized contract hedging errors."""

    complete_contracts_count: int
    incomplete_contracts_excluded: int
    hedging_error_mean: float | None
    hedging_error_rmse: float | None
    hedging_error_tail_loss: float | None
    tail_count: int
    mean_normalized_net_pnl: float | None


@dataclass(frozen=True)
class RunMetricsRow:
    """Single summary row per simulation run (policy, cost_bps, seed)."""

    policy: str
    cost_bps: int
    seed: int
    burn_in_days: int
    observed_days: int
    days_per_year: int
    hedger_count: int
    annualized_realized_volatility: float
    executed_shares_per_day: float
    hedger_executed_shares_per_day: float
    dollar_volume_per_day: float
    total_commission_paid: float
    net_pnl_observed: float
    net_pnl_per_hedger: float
    lifetime_net_pnl: float | None
    complete_contracts_count: int
    incomplete_contracts_excluded: int
    hedging_error_mean: float | None
    hedging_error_rmse: float | None
    hedging_error_tail_loss: float | None
    max_drawdown: float
    no_trade_day_share: float
    unfilled_hedge_demand_share: float | None
    limit_hit_count: int
    negative_equity_days: int
    min_hedger_equity: float | None
    mean_open_calls_shares: float
    zero_booked_fee_share: float | None
    schema_version: int = METRICS_SCHEMA_VERSION

    def to_dict(self) -> dict[str, Any]:
        """Serialize to a flat JSON-safe mapping with exact uint64 decimal seed string."""
        return {
            "schema_version": self.schema_version,
            "policy": self.policy,
            "cost_bps": self.cost_bps,
            "seed": str(self.seed),
            "burn_in_days": self.burn_in_days,
            "observed_days": self.observed_days,
            "days_per_year": self.days_per_year,
            "hedger_count": self.hedger_count,
            "annualized_realized_volatility": self.annualized_realized_volatility,
            "executed_shares_per_day": self.executed_shares_per_day,
            "hedger_executed_shares_per_day": self.hedger_executed_shares_per_day,
            "dollar_volume_per_day": self.dollar_volume_per_day,
            "total_commission_paid": self.total_commission_paid,
            "net_pnl_observed": self.net_pnl_observed,
            "net_pnl_per_hedger": self.net_pnl_per_hedger,
            "lifetime_net_pnl": self.lifetime_net_pnl,
            "complete_contracts_count": self.complete_contracts_count,
            "incomplete_contracts_excluded": self.incomplete_contracts_excluded,
            "hedging_error_mean": self.hedging_error_mean,
            "hedging_error_rmse": self.hedging_error_rmse,
            "hedging_error_tail_loss": self.hedging_error_tail_loss,
            "max_drawdown": self.max_drawdown,
            "no_trade_day_share": self.no_trade_day_share,
            "unfilled_hedge_demand_share": self.unfilled_hedge_demand_share,
            "limit_hit_count": self.limit_hit_count,
            "negative_equity_days": self.negative_equity_days,
            "min_hedger_equity": self.min_hedger_equity,
            "mean_open_calls_shares": self.mean_open_calls_shares,
            "zero_booked_fee_share": self.zero_booked_fee_share,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> RunMetricsRow:
        """Deserialize from a flat mapping produced by to_dict()."""
        raw_seed = payload["seed"]
        if isinstance(raw_seed, str):
            seed = int(raw_seed, 10)
        elif type(raw_seed) is int:
            seed = raw_seed
        else:
            raise ValueError("seed must be an int or base-10 string")
        return cls(
            schema_version=int(payload.get("schema_version", METRICS_SCHEMA_VERSION)),
            policy=str(payload["policy"]),
            cost_bps=_check_int(payload["cost_bps"], "cost_bps", minimum=0),
            seed=_check_int(seed, "seed", minimum=0),
            burn_in_days=_check_int(payload["burn_in_days"], "burn_in_days", minimum=0),
            observed_days=_check_int(payload["observed_days"], "observed_days", minimum=2),
            days_per_year=_check_int(payload["days_per_year"], "days_per_year", minimum=1),
            hedger_count=_check_int(payload["hedger_count"], "hedger_count", minimum=1),
            annualized_realized_volatility=_check_nonnegative(
                payload["annualized_realized_volatility"], "annualized_realized_volatility"
            ),
            executed_shares_per_day=_check_nonnegative(
                payload["executed_shares_per_day"], "executed_shares_per_day"
            ),
            hedger_executed_shares_per_day=_check_nonnegative(
                payload["hedger_executed_shares_per_day"], "hedger_executed_shares_per_day"
            ),
            dollar_volume_per_day=_check_nonnegative(
                payload["dollar_volume_per_day"], "dollar_volume_per_day"
            ),
            total_commission_paid=_check_nonnegative(
                payload["total_commission_paid"], "total_commission_paid"
            ),
            net_pnl_observed=_check_finite(payload["net_pnl_observed"], "net_pnl_observed"),
            net_pnl_per_hedger=_check_finite(payload["net_pnl_per_hedger"], "net_pnl_per_hedger"),
            lifetime_net_pnl=(
                None
                if payload.get("lifetime_net_pnl") is None
                else _check_finite(payload["lifetime_net_pnl"], "lifetime_net_pnl")
            ),
            complete_contracts_count=_check_int(
                payload["complete_contracts_count"], "complete_contracts_count", minimum=0
            ),
            incomplete_contracts_excluded=_check_int(
                payload["incomplete_contracts_excluded"], "incomplete_contracts_excluded", minimum=0
            ),
            hedging_error_mean=(
                None
                if payload.get("hedging_error_mean") is None
                else _check_finite(payload["hedging_error_mean"], "hedging_error_mean")
            ),
            hedging_error_rmse=(
                None
                if payload.get("hedging_error_rmse") is None
                else _check_nonnegative(payload["hedging_error_rmse"], "hedging_error_rmse")
            ),
            hedging_error_tail_loss=(
                None
                if payload.get("hedging_error_tail_loss") is None
                else _check_finite(payload["hedging_error_tail_loss"], "hedging_error_tail_loss")
            ),
            max_drawdown=_check_nonnegative(payload["max_drawdown"], "max_drawdown"),
            no_trade_day_share=_check_nonnegative(
                payload["no_trade_day_share"], "no_trade_day_share"
            ),
            unfilled_hedge_demand_share=(
                None
                if payload.get("unfilled_hedge_demand_share") is None
                else _check_nonnegative(
                    payload["unfilled_hedge_demand_share"], "unfilled_hedge_demand_share"
                )
            ),
            limit_hit_count=_check_int(payload["limit_hit_count"], "limit_hit_count", minimum=0),
            negative_equity_days=_check_int(
                payload["negative_equity_days"], "negative_equity_days", minimum=0
            ),
            min_hedger_equity=(
                None
                if payload.get("min_hedger_equity") is None
                else _check_finite(payload["min_hedger_equity"], "min_hedger_equity")
            ),
            mean_open_calls_shares=_check_nonnegative(
                payload["mean_open_calls_shares"], "mean_open_calls_shares"
            ),
            zero_booked_fee_share=(
                None
                if payload.get("zero_booked_fee_share") is None
                else _check_nonnegative(payload["zero_booked_fee_share"], "zero_booked_fee_share")
            ),
        )


def realized_volatility(close_prices: Sequence[float], *, days_per_year: int = 252) -> float:
    """Annualized sample SD of log returns over consecutive close prices.

    Expects M + 1 positive close prices [P_B, P_{B+1}, ..., P_{B+M}] to form M returns
    with M >= 2, returning sqrt(days_per_year) * sample_sd(r_t, ddof=1).
    """
    _check_int(days_per_year, "days_per_year", minimum=1)
    if len(close_prices) < 3:
        raise ValueError("at least 3 close prices (2 log returns) are required")
    prices = [_check_positive(price, "close_price") for price in close_prices]
    returns = [log(prices[i] / prices[i - 1]) for i in range(1, len(prices))]
    count = len(returns)
    mean_r = fsum(returns) / count
    sum_sq = fsum((r - mean_r) ** 2 for r in returns)
    return sqrt(days_per_year) * sqrt(sum_sq / (count - 1))


def aggregate_contract_errors(
    contracts: Sequence[ContractOutcome],
    *,
    burn_in_days: int = 500,
    observed_days: int = 2000,
    tail_fraction: float = 0.05,
) -> ContractErrorSummary:
    """Aggregate normalized hedging errors over complete contracts in [B, B + M].

    A contract is complete for the observed window iff settled is True,
    issue_step >= burn_in_days, and expiry_step <= burn_in_days + observed_days.
    Incomplete contracts are excluded and counted explicitly.
    When K == 0, error metrics are None (NA), never zero.
    """
    _check_int(burn_in_days, "burn_in_days", minimum=0)
    _check_int(observed_days, "observed_days", minimum=1)
    frac = _check_positive(tail_fraction, "tail_fraction")
    if frac > 1.0:
        raise ValueError("tail_fraction must be in (0, 1]")

    window_end = burn_in_days + observed_days
    normalized_pnls: list[float] = []
    epsilons: list[float] = []
    excluded = 0
    for contract in contracts:
        if (
            contract.settled
            and contract.issue_step >= burn_in_days
            and contract.expiry_step <= window_end
        ):
            notional = contract.size_shares * contract.issue_spot
            z_k = contract.net_pnl / notional
            if not isfinite(z_k):
                raise ValueError("normalized contract P&L exceeds finite float range")
            normalized_pnls.append(z_k)
            epsilons.append(-z_k)
        else:
            excluded += 1

    count = len(epsilons)
    if count == 0:
        return ContractErrorSummary(
            complete_contracts_count=0,
            incomplete_contracts_excluded=excluded,
            hedging_error_mean=None,
            hedging_error_rmse=None,
            hedging_error_tail_loss=None,
            tail_count=0,
            mean_normalized_net_pnl=None,
        )

    mean_eps = fsum(epsilons) / count
    rmse = sqrt(fsum(eps * eps for eps in epsilons) / count)
    tail_count = max(1, min(count, ceil(frac * count)))
    worst_eps = sorted(epsilons, reverse=True)[:tail_count]
    tail_loss = fsum(worst_eps) / tail_count
    mean_z = fsum(normalized_pnls) / count

    return ContractErrorSummary(
        complete_contracts_count=count,
        incomplete_contracts_excluded=excluded,
        hedging_error_mean=mean_eps,
        hedging_error_rmse=rmse,
        hedging_error_tail_loss=tail_loss,
        tail_count=tail_count,
        mean_normalized_net_pnl=mean_z,
    )


def summarize_run(
    *,
    policy: str,
    cost_bps: int,
    seed: int,
    daily_records: Sequence[DailyMarketRecord],
    contracts: Sequence[ContractOutcome] = (),
    burn_in_days: int = 500,
    observed_days: int = 2000,
    days_per_year: int = 252,
    hedger_count: int = 40,
) -> RunMetricsRow:
    """Collapse one simulation run into a single protocol metrics row with burn-in excluded."""
    if policy not in ALLOWED_POLICIES:
        raise ValueError(f"policy must be one of {sorted(ALLOWED_POLICIES)}")
    _check_int(cost_bps, "cost_bps", minimum=0)
    _check_int(seed, "seed", minimum=0)
    _check_int(burn_in_days, "burn_in_days", minimum=0)
    _check_int(observed_days, "observed_days", minimum=2)
    _check_int(days_per_year, "days_per_year", minimum=1)
    _check_int(hedger_count, "hedger_count", minimum=1)

    by_step: dict[int, DailyMarketRecord] = {}
    for rec in daily_records:
        if rec.step in by_step:
            raise ValueError(f"duplicate daily record for step {rec.step}")
        by_step[rec.step] = rec

    window_end = burn_in_days + observed_days
    if burn_in_days not in by_step:
        raise ValueError(f"missing anchor close record at burn-in boundary step {burn_in_days}")
    anchor = by_step[burn_in_days]

    observed: list[DailyMarketRecord] = []
    for step in range(burn_in_days + 1, window_end + 1):
        if step not in by_step:
            raise ValueError(f"missing observed daily record for step {step}")
        observed.append(by_step[step])

    close_series = [anchor.close_price, *(rec.close_price for rec in observed)]
    vol = realized_volatility(close_series, days_per_year=days_per_year)

    executed_shares = fsum(rec.bilateral_executed_shares for rec in observed) / observed_days
    hedger_shares = fsum(rec.hedger_executed_shares for rec in observed) / observed_days
    dollar_vol = (
        fsum(
            rec.dollar_volume
            if rec.dollar_volume is not None
            else rec.bilateral_executed_shares * rec.close_price
            for rec in observed
        )
        / observed_days
    )
    total_commission = fsum(rec.hedger_commission_paid for rec in observed)

    end_record = observed[-1]
    net_pnl_observed = end_record.hedger_total_equity - anchor.hedger_total_equity
    net_pnl_per_hedger = net_pnl_observed / hedger_count
    lifetime_net_pnl = (
        end_record.hedger_total_equity - by_step[0].hedger_total_equity if 0 in by_step else None
    )

    contract_summary = aggregate_contract_errors(
        contracts,
        burn_in_days=burn_in_days,
        observed_days=observed_days,
    )

    peak = anchor.close_price
    max_dd = 0.0
    for rec in observed:
        if rec.close_price > peak:
            peak = rec.close_price
        dd = 1.0 - rec.close_price / peak
        if dd > max_dd:
            max_dd = dd

    no_trade_days = sum(1 for rec in observed if rec.bilateral_executed_shares == 0.0)
    no_trade_share = no_trade_days / observed_days

    total_requested = fsum(rec.hedger_requested_shares for rec in observed)
    total_unfilled = fsum(rec.hedger_unfilled_shares for rec in observed)
    unfilled_share = total_unfilled / total_requested if total_requested > 0.0 else None

    limit_hits = sum(rec.limit_hits for rec in observed)

    min_equities = [rec.hedger_min_equity for rec in observed if rec.hedger_min_equity is not None]
    if min_equities:
        min_hedger_equity: float | None = min(min_equities)
        negative_equity_days = sum(1 for eq in min_equities if eq < 0.0)
    else:
        min_hedger_equity = None
        negative_equity_days = sum(1 for rec in observed if rec.hedger_total_equity < 0.0)

    mean_open_calls = fsum(rec.open_calls_shares for rec in observed) / observed_days

    pos_fills = sum(rec.positive_cost_fills for rec in observed)
    zero_fee_fills = sum(rec.zero_booked_fee_fills for rec in observed)
    zero_booked_fee_share = zero_fee_fills / pos_fills if cost_bps > 0 and pos_fills > 0 else None

    return RunMetricsRow(
        policy=policy,
        cost_bps=cost_bps,
        seed=seed,
        burn_in_days=burn_in_days,
        observed_days=observed_days,
        days_per_year=days_per_year,
        hedger_count=hedger_count,
        annualized_realized_volatility=vol,
        executed_shares_per_day=executed_shares,
        hedger_executed_shares_per_day=hedger_shares,
        dollar_volume_per_day=dollar_vol,
        total_commission_paid=total_commission,
        net_pnl_observed=net_pnl_observed,
        net_pnl_per_hedger=net_pnl_per_hedger,
        lifetime_net_pnl=lifetime_net_pnl,
        complete_contracts_count=contract_summary.complete_contracts_count,
        incomplete_contracts_excluded=contract_summary.incomplete_contracts_excluded,
        hedging_error_mean=contract_summary.hedging_error_mean,
        hedging_error_rmse=contract_summary.hedging_error_rmse,
        hedging_error_tail_loss=contract_summary.hedging_error_tail_loss,
        max_drawdown=max_dd,
        no_trade_day_share=no_trade_share,
        unfilled_hedge_demand_share=unfilled_share,
        limit_hit_count=limit_hits,
        negative_equity_days=negative_equity_days,
        min_hedger_equity=min_hedger_equity,
        mean_open_calls_shares=mean_open_calls,
        zero_booked_fee_share=zero_booked_fee_share,
    )


__all__ = [
    "ALLOWED_POLICIES",
    "METRICS_SCHEMA_VERSION",
    "ContractErrorSummary",
    "ContractOutcome",
    "DailyMarketRecord",
    "RunMetricsRow",
    "aggregate_contract_errors",
    "realized_volatility",
    "summarize_run",
]
