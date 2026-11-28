"""Substantive statistical tests for issue #13 (protocol #1 confirmatory & descriptive analysis)."""

from __future__ import annotations

import json
import random
from math import sqrt

import pytest

from finmath.metrics import RunMetricsRow
from finmath.statistics import (
    analyze_protocol_matrix,
    distribution_diagnostics,
    holm_adjust,
    paired_t_test,
    seed_block_bootstrap,
    seed_ols_slopes,
    seed_slope_test,
    student_t_ppf,
)


def _make_row(
    *,
    policy: str,
    cost_bps: int,
    seed: int,
    vol: float,
    turnover: float,
    commission: float = 10.0,
    net_pnl: float = 0.0,
    rmse: float | None = 0.02,
) -> RunMetricsRow:
    return RunMetricsRow(
        policy=policy,
        cost_bps=cost_bps,
        seed=seed,
        burn_in_days=500,
        observed_days=2000,
        days_per_year=252,
        hedger_count=40,
        annualized_realized_volatility=vol,
        executed_shares_per_day=turnover,
        hedger_executed_shares_per_day=turnover * 0.4,
        dollar_volume_per_day=turnover * 100.0,
        total_commission_paid=commission,
        net_pnl_observed=net_pnl,
        net_pnl_per_hedger=net_pnl / 40.0,
        lifetime_net_pnl=net_pnl,
        complete_contracts_count=100 if rmse is not None else 0,
        incomplete_contracts_excluded=4,
        hedging_error_mean=0.001 if rmse is not None else None,
        hedging_error_rmse=rmse,
        hedging_error_tail_loss=0.05 if rmse is not None else None,
        max_drawdown=0.08,
        no_trade_day_share=0.01,
        unfilled_hedge_demand_share=0.02,
        limit_hit_count=0,
        negative_equity_days=0,
        min_hedger_equity=10.0,
        mean_open_calls_shares=400.0,
        zero_booked_fee_share=0.0 if cost_bps > 0 else None,
    )


def test_holm_adjust_preserves_order_and_enforces_stepdown_monotonicity() -> None:
    # Sorted: 0.01 (*3=0.03), 0.02 (*2=0.04), 0.04 (*1=0.04) -> [0.04, 0.03, 0.04]
    assert holm_adjust([0.04, 0.01, 0.02]) == pytest.approx((0.04, 0.03, 0.04))
    # Monotonicity clamp: 0.02*3=0.06 > 0.025*2=0.05 -> second gets 0.06
    assert holm_adjust([0.02, 0.025, 0.5]) == pytest.approx((0.06, 0.06, 0.5))
    # Upper bound at 1.0
    assert holm_adjust([0.4, 0.6, 0.8]) == pytest.approx((1.0, 1.0, 1.0))


def test_paired_t_test_matches_hand_calculation_and_coverage_types() -> None:
    bs = [0.20, 0.22, 0.19, 0.21, 0.23]
    dl = [0.18, 0.19, 0.18, 0.17, 0.20]
    diffs = [d - b for d, b in zip(dl, bs, strict=True)]  # [-0.02, -0.03, -0.01, -0.04, -0.03]
    mean_d = sum(diffs) / 5
    sd_d = sqrt(sum((x - mean_d) ** 2 for x in diffs) / 4)
    se_d = sd_d / sqrt(5)

    res = paired_t_test(dl, bs, hypothesis_id="H1", endpoint="V", alternative="less", family_size=3)
    assert res.n_seeds == 5
    assert res.df == 4
    assert res.estimate == pytest.approx(mean_d)
    assert res.sd == pytest.approx(sd_d)
    assert res.se == pytest.approx(se_d)
    assert res.statistic == pytest.approx(mean_d / se_d)
    assert res.estimable is True
    assert res.p_raw < 0.005
    assert res.relative_change == pytest.approx(mean_d / (sum(bs) / 5))
    assert res.standardized_dz == pytest.approx(mean_d / sd_d)

    t95 = student_t_ppf(0.975, 4)
    assert res.t_ci.coverage_type == "marginal"
    assert res.t_ci.level == pytest.approx(0.95)
    assert res.t_ci.lower == pytest.approx(mean_d - t95 * se_d)
    assert res.t_ci.upper == pytest.approx(mean_d + t95 * se_d)

    t_bonf = student_t_ppf(1.0 - (0.05 / 3) / 2.0, 4)
    assert res.bonferroni_t_ci.coverage_type == "familywise-bonferroni"
    assert res.bonferroni_t_ci.level == pytest.approx(1.0 - 0.05 / 3)
    assert res.bonferroni_t_ci.lower == pytest.approx(mean_d - t_bonf * se_d)
    assert res.bonferroni_t_ci.upper == pytest.approx(mean_d + t_bonf * se_d)
    assert res.bonferroni_t_ci.lower < res.t_ci.lower


def test_degenerate_zero_variance_and_zero_comparator_mean_follow_protocol() -> None:
    # All differences exactly zero -> statistic=0, p=1, t-CI undefined
    zero_res = paired_t_test([0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
    assert zero_res.statistic == 0.0
    assert zero_res.p_raw == 1.0
    assert zero_res.estimable is True
    assert zero_res.relative_change is None  # zero comparator mean -> NA, not inf
    assert zero_res.standardized_dz is None
    assert zero_res.t_ci.lower is None and zero_res.t_ci.degenerate is True

    # Constant nonzero effect across seeds -> non-estimable, p_raw=1 (never artificial p=0)
    const_res = paired_t_test([0.15, 0.15, 0.15, 0.15], [0.20, 0.20, 0.20, 0.20])
    assert const_res.estimate == pytest.approx(-0.05)
    assert const_res.statistic is None
    assert const_res.estimable is False
    assert const_res.p_raw == 1.0
    assert const_res.degenerate_reason == "zero_variance_nonzero_effect"
    assert const_res.t_ci.lower is None and const_res.t_ci.degenerate is True


def test_seed_slope_test_recovers_exact_within_seed_ols_slopes_and_seed_df() -> None:
    # Protocol grid x = [0, 0.5, 1, 2, 4] has mean(x)=1.5 and sum((x-1.5)^2)=10.
    x = [0.0, 0.5, 1.0, 2.0, 4.0]
    true_slopes = [-0.010, -0.014, -0.008, -0.012, -0.016, -0.010]
    intercepts = [0.005, -0.002, 0.001, 0.003, -0.004, 0.000]
    diffs_by_seed = [[a + b * xj for xj in x] for a, b in zip(intercepts, true_slopes, strict=True)]

    slopes = seed_ols_slopes(diffs_by_seed, x)
    assert slopes == pytest.approx(true_slopes, abs=1e-14)

    res = seed_slope_test(diffs_by_seed, x)
    expected_mean = sum(true_slopes) / len(true_slopes)
    expected_sd = sqrt(sum((b - expected_mean) ** 2 for b in true_slopes) / (len(true_slopes) - 1))
    assert res.n_seeds == 6
    assert res.df == 5  # seed-level df, never 6*5 - 2
    assert res.estimate == pytest.approx(expected_mean)
    assert res.se == pytest.approx(expected_sd / sqrt(6))
    assert res.p_raw < 0.001


def test_distribution_diagnostics_detects_outlier_and_leave_one_seed_out_sign_flip() -> None:
    # Nine mildly positive seeds plus one huge negative outlier that flips the overall mean < 0.
    seeds = list(range(1000, 1010))
    values = [0.01] * 9 + [-0.20]
    diag = distribution_diagnostics(values, seeds, name="H1: D_V(c0)", bins=5)

    assert diag.n_seeds == 10
    assert diag.mean < 0.0
    assert diag.min_seed == 1009
    assert diag.most_influential_seed == 1009
    assert 1009 in diag.outlier_seeds
    assert diag.sign_flips_on_leave_one_out is True
    assert diag.leave_one_out_max == pytest.approx(0.01)
    assert diag.skewness is not None and diag.skewness < 0.0
    assert diag.excess_kurtosis is not None
    assert len(diag.qq_points) == 10
    assert sum(diag.histogram_counts) == 10


def test_seed_block_bootstrap_resamples_whole_blocks_deterministically() -> None:
    rng = random.Random(42)
    x = [rng.gauss(-0.02, 0.01) for _ in range(30)]
    neg_x = [-v for v in x]
    const = [0.05] * 30

    b1 = seed_block_bootstrap(
        {"x": x, "neg_x": neg_x, "const": const}, resamples=2000, bootstrap_seed=71001
    )
    b2 = seed_block_bootstrap(
        {"x": x, "neg_x": neg_x, "const": const}, resamples=2000, bootstrap_seed=71001
    )
    assert b1 == b2
    # Whole-seed-block resampling guarantees exact mirror symmetry for neg_x = -x
    assert b1["neg_x"].lower == pytest.approx(-b1["x"].upper, abs=1e-14)
    assert b1["neg_x"].upper == pytest.approx(-b1["x"].lower, abs=1e-14)
    # Zero variance -> degenerate point bootstrap, no 95% coverage claim
    assert b1["const"].degenerate is True
    assert b1["const"].lower == pytest.approx(0.05)
    assert b1["const"].upper == pytest.approx(0.05)


def test_analyze_protocol_matrix_full_4x5x50_synthetic_and_rejects_incomplete() -> None:
    rng = random.Random(12345)
    seeds = list(range(1000, 1050))
    costs = (0, 5, 10, 20, 40)
    rows: list[RunMetricsRow] = []

    for seed in seeds:
        base_vol = 0.20 + rng.gauss(0.0, 0.02)
        base_turn = 500.0 + rng.gauss(0.0, 40.0)
        seed_slope = -0.008 + rng.gauss(0.0, 0.003)
        for cost in costs:
            x = cost / 10.0
            bs_vol = max(0.05, base_vol + 0.002 * x + rng.gauss(0.0, 0.002))
            dl_vol = max(0.05, bs_vol - 0.015 + seed_slope * x + rng.gauss(0.0, 0.003))
            leland_vol = max(0.05, bs_vol - 0.005)
            band_vol = max(0.05, bs_vol - 0.008)

            bs_turn = max(10.0, base_turn - 5.0 * x + rng.gauss(0.0, 5.0))
            dl_turn = max(10.0, bs_turn - 80.0 + rng.gauss(0.0, 10.0))
            leland_turn = max(10.0, bs_turn - 20.0)
            band_turn = max(10.0, bs_turn - 50.0)

            rows.append(
                _make_row(policy="bs", cost_bps=cost, seed=seed, vol=bs_vol, turnover=bs_turn)
            )
            rows.append(
                _make_row(policy="dl", cost_bps=cost, seed=seed, vol=dl_vol, turnover=dl_turn)
            )
            rows.append(
                _make_row(
                    policy="leland", cost_bps=cost, seed=seed, vol=leland_vol, turnover=leland_turn
                )
            )
            rows.append(
                _make_row(policy="band", cost_bps=cost, seed=seed, vol=band_vol, turnover=band_turn)
            )

    assert len(rows) == 1000
    result = analyze_protocol_matrix(
        rows, expected_seeds=seeds, bootstrap_resamples=1000, bootstrap_seed=71001
    )

    assert result.n_seeds == 50
    assert result.multiple_testing == "holm-3-hypotheses"
    assert result.bootstrap_unit == "whole-market-seed-block"
    assert [h.hypothesis_id for h in result.hypotheses] == ["H1", "H2", "H3"]
    for h in result.hypotheses:
        assert h.df == 49
        assert h.estimate < 0.0
        assert h.p_adjusted is not None and h.p_adjusted < 0.05
        assert h.rejected is True
        assert h.bootstrap_ci is not None and h.bootstrap_ci.upper < 0.0

    # 3 comparators * 5 costs * 5 descriptive metrics = 75 rows
    assert len(result.descriptive_contrasts) == 75
    assert set(result.diagnostics) == {"H1", "H2", "H3"}

    # Full JSON serialization without NaN/Inf
    encoded = json.dumps(result.to_dict(), allow_nan=False)
    assert '"multiple_testing": "holm-3-hypotheses"' in encoded

    # Removing one cell must fail loudly (no silent complete-case filtering)
    with pytest.raises(ValueError, match="incomplete matrix"):
        analyze_protocol_matrix(rows[:-1], expected_seeds=seeds, bootstrap_resamples=100)

    # Duplicate run key must fail loudly
    with pytest.raises(ValueError, match="duplicate"):
        analyze_protocol_matrix([*rows, rows[0]], expected_seeds=seeds, bootstrap_resamples=100)


def test_decimal_constant_differences_do_not_create_significance():
    result = paired_t_test([0.0] * 50, [0.007] * 50)
    assert result.sd == 0
    assert result.estimable is False
    assert result.p_raw == 1
    curves = [[0.1] * 5] * 25 + [[0.2] * 5] * 25
    assert seed_ols_slopes(curves, [0, 0.5, 1, 2, 4]) == (0.0,) * 50
    result = seed_slope_test(curves, [0, 0.5, 1, 2, 4])
    assert result.estimate == 0
    assert result.p_raw == 1


@pytest.mark.parametrize("missing", ["seed", "band", "leland", "cell"])
def test_final_matrix_requires_manifest_blocks_and_all_policies(missing):
    seeds = list(range(1000, 1050))
    rows = [
        _make_row(policy=p, cost_bps=c, seed=s, vol=0.2, turnover=100)
        for p in ("bs", "leland", "band", "dl")
        for c in (0, 5, 10, 20, 40)
        for s in seeds
    ]
    if missing == "seed":
        rows = [r for r in rows if r.seed != seeds[0]]
    elif missing == "cell":
        rows.pop()
    else:
        rows = [r for r in rows if r.policy != missing]
    with pytest.raises(ValueError, match="incomplete matrix"):
        analyze_protocol_matrix(rows, expected_seeds=seeds, bootstrap_resamples=10)


def test_fixture_mode_is_explicit_and_final_requires_manifest():
    rows = [
        _make_row(policy=p, cost_bps=c, seed=s, vol=0.2, turnover=100)
        for p in ("bs", "dl")
        for c in (0, 5, 10, 20, 40)
        for s in (1000, 1001)
    ]
    with pytest.raises(ValueError, match="expected_seeds"):
        analyze_protocol_matrix(rows)
    with pytest.raises(ValueError, match="50"):
        analyze_protocol_matrix(rows, expected_seeds=[1000, 1001])
    result = analyze_protocol_matrix(rows, mode="fixture", bootstrap_resamples=10)
    assert result.to_dict()["analysis_mode"] == "fixture"
