"""Hand-calculated and invariant tests for run-level metrics (issue #13)."""

from __future__ import annotations

import json
from math import log, sqrt

import pytest

from finmath.metrics import (
    ContractOutcome,
    DailyMarketRecord,
    RunMetricsRow,
    aggregate_contract_errors,
    realized_volatility,
    summarize_run,
)


def test_summarize_run_excludes_burn_in_and_matches_hand_calculation() -> None:
    # Burn-in days (steps 0 and 1) contain extreme prices/volumes/fees/limit hits
    # that must not leak into observed metrics for steps 3..6 (B=2, M=4).
    daily = [
        DailyMarketRecord(
            step=0,
            close_price=50.0,
            bilateral_executed_shares=9999.0,
            hedger_executed_shares=5000.0,
            hedger_commission_paid=777.0,
            hedger_total_equity=1000.0,
            hedger_min_equity=-500.0,
            hedger_requested_shares=5000.0,
            hedger_unfilled_shares=2500.0,
            limit_hits=99,
            open_calls_shares=10_000.0,
            positive_cost_fills=100,
            zero_booked_fee_fills=50,
        ),
        DailyMarketRecord(
            step=1,
            close_price=250.0,
            bilateral_executed_shares=8888.0,
            hedger_executed_shares=4000.0,
            hedger_commission_paid=666.0,
            hedger_total_equity=900.0,
            hedger_min_equity=-400.0,
            limit_hits=88,
            open_calls_shares=8000.0,
        ),
        # Anchor step B=2: sets P_B=100 and equity_B=1200; its volumes/fees are still burn-in.
        DailyMarketRecord(
            step=2,
            close_price=100.0,
            bilateral_executed_shares=7777.0,
            hedger_executed_shares=3000.0,
            hedger_commission_paid=555.0,
            hedger_total_equity=1200.0,
            hedger_min_equity=-300.0,
            limit_hits=77,
            open_calls_shares=6000.0,
        ),
        # Observed window: steps 3, 4, 5, 6 (M=4)
        DailyMarketRecord(
            step=3,
            close_price=110.0,
            bilateral_executed_shares=100.0,
            hedger_executed_shares=60.0,
            hedger_commission_paid=10.0,
            hedger_total_equity=1220.0,
            hedger_min_equity=5.0,
            hedger_requested_shares=80.0,
            hedger_unfilled_shares=20.0,
            limit_hits=1,
            open_calls_shares=400.0,
            positive_cost_fills=10,
            zero_booked_fee_fills=1,
        ),
        # No-trade day: close stays 110.0, r_4 = 0 remains in volatility series
        DailyMarketRecord(
            step=4,
            close_price=110.0,
            bilateral_executed_shares=0.0,
            hedger_executed_shares=0.0,
            hedger_commission_paid=0.0,
            hedger_total_equity=1220.0,
            hedger_min_equity=-2.0,
            hedger_requested_shares=0.0,
            hedger_unfilled_shares=0.0,
            limit_hits=0,
            open_calls_shares=400.0,
            positive_cost_fills=0,
            zero_booked_fee_fills=0,
        ),
        DailyMarketRecord(
            step=5,
            close_price=99.0,
            bilateral_executed_shares=200.0,
            hedger_executed_shares=120.0,
            hedger_commission_paid=25.0,
            hedger_total_equity=1150.0,
            hedger_min_equity=-10.0,
            hedger_requested_shares=120.0,
            hedger_unfilled_shares=0.0,
            limit_hits=2,
            open_calls_shares=300.0,
            positive_cost_fills=10,
            zero_booked_fee_fills=0,
        ),
        DailyMarketRecord(
            step=6,
            close_price=105.0,
            bilateral_executed_shares=100.0,
            hedger_executed_shares=20.0,
            hedger_commission_paid=5.0,
            hedger_total_equity=1180.0,
            hedger_min_equity=1.0,
            hedger_requested_shares=0.0,
            hedger_unfilled_shares=0.0,
            limit_hits=0,
            open_calls_shares=500.0,
            positive_cost_fills=5,
            zero_booked_fee_fills=1,
        ),
    ]

    contracts = [
        # Excluded: issued before burn-in boundary B=2
        ContractOutcome(issue_step=1, expiry_step=4, size_shares=10, issue_spot=100, net_pnl=-50),
        # Included complete #1: notional = 1000, z = +0.02, eps = -0.02
        ContractOutcome(issue_step=2, expiry_step=4, size_shares=10, issue_spot=100, net_pnl=20.0),
        # Included complete #2: notional = 2000, z = -0.05, eps = +0.05
        ContractOutcome(
            issue_step=3, expiry_step=6, size_shares=20, issue_spot=100, net_pnl=-100.0
        ),
        # Excluded: expires after observed window end B+M=6
        ContractOutcome(issue_step=5, expiry_step=7, size_shares=10, issue_spot=100, net_pnl=-10.0),
        # Excluded: unsettled
        ContractOutcome(
            issue_step=3,
            expiry_step=5,
            size_shares=10,
            issue_spot=100,
            net_pnl=-10.0,
            settled=False,
        ),
    ]

    row = summarize_run(
        policy="dl",
        cost_bps=10,
        seed=1001,
        daily_records=daily,
        contracts=contracts,
        burn_in_days=2,
        observed_days=4,
        days_per_year=252,
        hedger_count=40,
    )

    returns = [log(110 / 100), 0.0, log(99 / 110), log(105 / 99)]
    mean_r = sum(returns) / 4
    expected_vol = sqrt(252) * sqrt(sum((r - mean_r) ** 2 for r in returns) / 3)

    assert row.annualized_realized_volatility == pytest.approx(expected_vol, abs=1e-14)
    assert row.executed_shares_per_day == pytest.approx((100 + 0 + 200 + 100) / 4)
    assert row.hedger_executed_shares_per_day == pytest.approx((60 + 0 + 120 + 20) / 4)
    assert row.dollar_volume_per_day == pytest.approx((100 * 110 + 0 + 200 * 99 + 100 * 105) / 4)
    assert row.total_commission_paid == pytest.approx(40.0)
    assert row.net_pnl_observed == pytest.approx(1180.0 - 1200.0)
    assert row.net_pnl_per_hedger == pytest.approx(-20.0 / 40)
    assert row.lifetime_net_pnl == pytest.approx(1180.0 - 1000.0)

    # Contracts: 2 complete, 3 excluded; eps = [-0.02, 0.05]
    assert row.complete_contracts_count == 2
    assert row.incomplete_contracts_excluded == 3
    assert row.hedging_error_mean == pytest.approx((-0.02 + 0.05) / 2)
    assert row.hedging_error_rmse == pytest.approx(sqrt((0.02**2 + 0.05**2) / 2))
    assert row.hedging_error_tail_loss == pytest.approx(0.05)  # ceil(0.05 * 2) == 1

    # Drawdown: peak 110 -> trough 99 -> 1 - 99/110 = 0.1
    assert row.max_drawdown == pytest.approx(0.1)
    assert row.no_trade_day_share == pytest.approx(0.25)
    assert row.unfilled_hedge_demand_share == pytest.approx(20.0 / 200.0)
    assert row.limit_hit_count == 3
    assert row.negative_equity_days == 2
    assert row.min_hedger_equity == pytest.approx(-10.0)
    assert row.mean_open_calls_shares == pytest.approx((400 + 400 + 300 + 500) / 4)
    assert row.zero_booked_fee_share == pytest.approx(2 / 25)


def test_zero_complete_contracts_returns_na_not_zero_risk() -> None:
    incomplete = [
        ContractOutcome(issue_step=0, expiry_step=505, size_shares=100, issue_spot=100, net_pnl=10),
        ContractOutcome(
            issue_step=2490, expiry_step=2510, size_shares=100, issue_spot=100, net_pnl=-5
        ),
    ]
    summary = aggregate_contract_errors(incomplete, burn_in_days=500, observed_days=2000)
    assert summary.complete_contracts_count == 0
    assert summary.incomplete_contracts_excluded == 2
    assert summary.hedging_error_mean is None
    assert summary.hedging_error_rmse is None
    assert summary.hedging_error_tail_loss is None
    assert summary.tail_count == 0
    assert summary.mean_normalized_net_pnl is None


def test_contract_tail_loss_uses_ceil_of_five_percent() -> None:
    # 25 complete contracts -> ceil(0.05 * 25) = 2 worst shortfalls.
    contracts = [
        ContractOutcome(
            issue_step=500 + i,
            expiry_step=520 + i,
            size_shares=10,
            issue_spot=10.0,
            net_pnl=-float(i),  # notional=100 -> epsilon = i / 100
        )
        for i in range(1, 26)
    ]
    summary = aggregate_contract_errors(contracts, burn_in_days=500, observed_days=2000)
    assert summary.complete_contracts_count == 25
    assert summary.tail_count == 2
    assert summary.hedging_error_tail_loss == pytest.approx((0.25 + 0.24) / 2)


def test_row_serialization_preserves_exact_uint64_seed_and_rejects_nan() -> None:
    big_seed = (1 << 63) + 987_654_321
    daily = [
        DailyMarketRecord(step=0, close_price=100.0, bilateral_executed_shares=10.0),
        DailyMarketRecord(step=1, close_price=101.0, bilateral_executed_shares=10.0),
        DailyMarketRecord(step=2, close_price=102.0, bilateral_executed_shares=10.0),
    ]
    row = summarize_run(
        policy="bs",
        cost_bps=0,
        seed=big_seed,
        daily_records=daily,
        burn_in_days=0,
        observed_days=2,
    )
    payload = row.to_dict()
    assert payload["seed"] == str(big_seed)
    encoded = json.dumps(payload, allow_nan=False)
    restored = RunMetricsRow.from_dict(json.loads(encoded))
    assert restored == row
    assert restored.seed == big_seed
    assert restored.zero_booked_fee_share is None


def test_validation_rejects_missing_or_duplicate_steps_and_bad_inputs() -> None:
    with pytest.raises(ValueError, match="at least 3"):
        realized_volatility([100.0, 101.0])
    with pytest.raises(ValueError, match="strictly positive"):
        DailyMarketRecord(step=0, close_price=0.0, bilateral_executed_shares=1.0)
    with pytest.raises(ValueError, match="exceed"):
        DailyMarketRecord(
            step=0,
            close_price=100.0,
            bilateral_executed_shares=1.0,
            hedger_requested_shares=5.0,
            hedger_unfilled_shares=6.0,
        )
    valid = [
        DailyMarketRecord(step=0, close_price=100.0, bilateral_executed_shares=1.0),
        DailyMarketRecord(step=1, close_price=101.0, bilateral_executed_shares=1.0),
    ]
    with pytest.raises(ValueError, match="missing observed"):
        summarize_run(
            policy="bs",
            cost_bps=10,
            seed=1000,
            daily_records=valid,
            burn_in_days=0,
            observed_days=2,
        )
    dup = [*valid, DailyMarketRecord(step=1, close_price=102.0, bilateral_executed_shares=1.0)]
    with pytest.raises(ValueError, match="duplicate"):
        summarize_run(
            policy="bs",
            cost_bps=10,
            seed=1000,
            daily_records=dup,
            burn_in_days=0,
            observed_days=2,
        )


@pytest.mark.parametrize("minima", [(None, None), (-10.0, None), (-10.0, 2.0)])
def test_equity_diagnostics_require_daily_minimum_coverage(minima):
    daily = [
        DailyMarketRecord(
            step=i,
            close_price=100,
            bilateral_executed_shares=0,
            hedger_executed_shares=0,
            hedger_commission_paid=0,
            hedger_total_equity=10,
            hedger_min_equity=None if i == 0 else minima[i - 1],
        )
        for i in range(3)
    ]
    row = summarize_run(
        policy="bs",
        cost_bps=10,
        seed=1000,
        daily_records=daily,
        burn_in_days=0,
        observed_days=2,
        hedger_count=2,
    )
    assert row.equity_observed_days == sum(m is not None for m in minima)
    if None in minima:
        assert row.negative_equity_days is None
        assert row.min_hedger_equity is None
    else:
        assert row.negative_equity_days == 1
        assert row.min_hedger_equity == -10
    assert RunMetricsRow.from_dict(row.to_dict()) == row
