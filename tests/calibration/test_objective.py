# AI assistance: OpenAI ChatGPT. The reference and simulated statistics are synthetic.
from copy import deepcopy

import pytest

from finmath.calibration.objective import evaluate, evaluate_holdout, rank_candidates


def snapshot(volatility: float = 0.2) -> dict:
    return {
        "config_sha256": "c" * 64,
        "return_count": 30,
        "annualized_volatility": volatility,
        "daily_volatility": 0.01,
        "log_return_quantiles": {"p05": -0.02, "p95": 0.015},
        "acf_absolute_log_returns": {"1": 0.4, "5": 0.3, "20": 0.2},
    }


def test_uses_equal_seed_weights_and_records_moment_differences() -> None:
    score = evaluate(
        snapshot(), {"10000": snapshot(0.1), "10001": snapshot(0.3)}, ["10000", "10001"]
    )
    assert score["loss"] == pytest.approx(0)
    assert score["simulated_seed_count"] == 2
    assert score["simulated_mean"]["annualized_volatility"] == pytest.approx(0.2)
    assert score["simulated_minus_target"]["abs_return_acf_20"] == pytest.approx(0)


def test_grid_ranks_on_calibration_only() -> None:
    meta = {
        "source": {"raw_sha256": "a" * 64},
        "splits": {"calibration": snapshot(), "holdout": snapshot(0.6)},
    }
    seed = "10000"
    grid = {
        "seeds": [seed],
        "candidates": {
            "second": {seed: snapshot(0.6)},
            "first": {seed: snapshot()},
            "incomplete": {},
        },
    }
    report = rank_candidates(meta, grid)
    assert report["selected"] == "first"
    assert report["failures"] == {
        "incomplete": "candidate must contain one summary per reference-market seed"
    }
    meta["splits"]["holdout"] = None
    assert rank_candidates(meta, grid) == report


def test_undefined_acf_is_a_recorded_failure() -> None:
    reference = snapshot()
    flat_series = deepcopy(reference)
    flat_series["annualized_volatility"] = 0.0
    flat_series["acf_absolute_log_returns"]["1"] = None
    report = rank_candidates(
        {"source": {"raw_sha256": "a" * 64}, "splits": {"calibration": reference}},
        {"seeds": ["10000"], "candidates": {"flat": {"10000": flat_series}}},
    )
    assert report["selected"] is None
    assert report["failures"] == {"flat": "missing or non-finite abs_return_acf_1"}


def test_holdout_requires_the_frozen_selection_and_independent_seeds() -> None:
    meta = {"source": {"raw_sha256": "a" * 64}, "splits": {"holdout": snapshot()}}
    selection = {
        "reference_sha256": "a" * 64,
        "selected": "base",
        "selected_config_sha256": "c" * 64,
        "seeds": ["10000"],
    }
    holdout = {"candidate": "base", "seeds": ["10050"], "simulations": {"10050": snapshot()}}
    assert evaluate_holdout(meta, holdout, selection)["loss"] == pytest.approx(0)
    with pytest.raises(ValueError, match="selected calibration candidate"):
        evaluate_holdout(meta, {**holdout, "candidate": "other"}, selection)
    with pytest.raises(ValueError, match="seeds overlap"):
        evaluate_holdout(meta, {**holdout, "seeds": ["10000"]}, selection)
    with pytest.raises(ValueError, match="same source snapshot"):
        evaluate_holdout(meta, holdout, {**selection, "reference_sha256": "b" * 64})


def test_rejects_noncanonical_seed_aliases() -> None:
    meta = {
        "splits": {"calibration": snapshot(), "holdout": snapshot()},
        "source": {"raw_sha256": "a" * 64},
    }
    summary = snapshot()
    with pytest.raises(ValueError, match="unsigned decimal seed strings"):
        rank_candidates(meta, {"seeds": ["10000", "010000"], "candidates": {"base": {}}})
    selection = rank_candidates(
        meta, {"seeds": ["10000"], "candidates": {"base": {"10000": summary}}}
    )
    with pytest.raises(ValueError, match="unsigned decimal seed strings"):
        evaluate_holdout(
            meta,
            {"candidate": "base", "seeds": ["010000"], "simulations": {"010000": summary}},
            selection,
        )
    with pytest.raises(ValueError, match="one summary per"):
        evaluate(summary, {"010000": summary}, ["10000"])


def test_requires_same_config_across_fit_seeds_and_during_holdout() -> None:
    good, changed = snapshot(), {**snapshot(), "config_sha256": "d" * 64}
    meta = {"source": {"raw_sha256": "a" * 64}, "splits": {"calibration": good, "holdout": good}}
    with pytest.raises(ValueError, match="different base configurations"):
        evaluate(good, {"10000": good, "10001": changed}, ["10000", "10001"])
    selection = rank_candidates(meta, {"seeds": ["10000"], "candidates": {"base": {"10000": good}}})
    with pytest.raises(ValueError, match="SHA256 differs"):
        evaluate_holdout(
            meta,
            {"candidate": "base", "seeds": ["10050"], "simulations": {"10050": changed}},
            selection,
        )
