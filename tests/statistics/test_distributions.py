"""Analytic and reference-value checks for exact stdlib Student-t functions."""

from __future__ import annotations

from math import atan, pi, sqrt, tan

import pytest

from finmath.statistics.distributions import (
    regularized_incomplete_beta,
    student_t_cdf,
    student_t_ppf,
)


@pytest.mark.parametrize("t_val", [-10.0, -2.5, -1.0, -0.25, 0.0, 0.25, 1.0, 2.5, 10.0])
def test_student_t_cdf_matches_exact_cauchy_and_df2_formulas(t_val: float) -> None:
    # df = 1 is the standard Cauchy distribution
    expected_df1 = 0.5 + atan(t_val) / pi
    assert student_t_cdf(t_val, 1) == pytest.approx(expected_df1, abs=1e-13)

    # df = 2 has closed-form CDF: 0.5 + t / (2 * sqrt(2 + t^2))
    expected_df2 = 0.5 + t_val / (2.0 * sqrt(2.0 + t_val * t_val))
    assert student_t_cdf(t_val, 2) == pytest.approx(expected_df2, abs=1e-14)


@pytest.mark.parametrize(
    ("probability", "df", "expected"),
    [
        (0.975, 1, tan(0.475 * pi)),
        (0.975, 2, 0.95 * sqrt(2.0 / (1.0 - 0.95**2))),
        (0.975, 10, 2.228138851986274),
        (0.975, 49, 2.0095752371292352),
        (1.0 - 0.05 / 6.0, 49, 2.478953032321815),
    ],
)
def test_student_t_ppf_matches_reference_critical_values(
    probability: float, df: int, expected: float
) -> None:
    actual = student_t_ppf(probability, df)
    assert actual == pytest.approx(expected, abs=1e-11)
    assert student_t_ppf(1.0 - probability, df) == pytest.approx(-actual, abs=1e-12)
    assert student_t_cdf(actual, df) == pytest.approx(probability, abs=1e-12)


@pytest.mark.parametrize("df", [1, 2, 5, 15, 49, 200])
@pytest.mark.parametrize("p", [0.001, 0.025, 0.1, 0.5, 0.9, 0.975, 0.999])
def test_student_t_cdf_and_ppf_are_inverses(df: int, p: float) -> None:
    q = student_t_ppf(p, df)
    assert student_t_cdf(q, df) == pytest.approx(p, abs=1e-12)


def test_boundary_and_invalid_distribution_inputs() -> None:
    assert regularized_incomplete_beta(0.0, 2.0, 3.0) == 0.0
    assert regularized_incomplete_beta(1.0, 2.0, 3.0) == 1.0
    assert student_t_cdf(float("-inf"), 49) == 0.0
    assert student_t_cdf(float("inf"), 49) == 1.0

    with pytest.raises(ValueError):
        student_t_cdf(0.0, 0)
    with pytest.raises(ValueError):
        student_t_cdf(float("nan"), 49)
    with pytest.raises(ValueError):
        student_t_ppf(0.0, 49)
    with pytest.raises(ValueError):
        student_t_ppf(1.0, 49)
