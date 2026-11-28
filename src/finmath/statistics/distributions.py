"""Exact Student-t CDF and quantile functions using Python stdlib only.

Regularized incomplete beta uses the continued fraction DLMF 8.17.22
(modified Lentz's method, Press et al. Numerical Recipes section 6.4).
"""

from __future__ import annotations

from math import exp, isfinite, lgamma, log

_FPMIN = 1e-300
_EPS = 1e-15
_MAX_ITER = 500


def _betacf(a: float, b: float, x: float) -> float:
    """Continued fraction for regularized incomplete beta I_x(a, b)."""
    qab = a + b
    qap = a + 1.0
    qam = a - 1.0
    c = 1.0
    d = 1.0 - qab * x / qap
    if abs(d) < _FPMIN:
        d = _FPMIN
    d = 1.0 / d
    h = d
    for m in range(1, _MAX_ITER + 1):
        m2 = 2 * m
        # Even step
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < _FPMIN:
            d = _FPMIN
        c = 1.0 + aa / c
        if abs(c) < _FPMIN:
            c = _FPMIN
        d = 1.0 / d
        h *= d * c
        # Odd step
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < _FPMIN:
            d = _FPMIN
        c = 1.0 + aa / c
        if abs(c) < _FPMIN:
            c = _FPMIN
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < _EPS:
            return h
    raise RuntimeError("incomplete beta continued fraction failed to converge")


def regularized_incomplete_beta(x: float, a: float, b: float) -> float:
    """Evaluate the regularized incomplete beta function I_x(a, b)."""
    if not (isfinite(x) and 0.0 <= x <= 1.0):
        raise ValueError("x must be finite and in [0, 1]")
    if not (isfinite(a) and a > 0.0 and isfinite(b) and b > 0.0):
        raise ValueError("a and b must be strictly positive finite numbers")
    if x == 0.0:
        return 0.0
    if x == 1.0:
        return 1.0
    ln_beta = lgamma(a) + lgamma(b) - lgamma(a + b)
    front = exp(a * log(x) + b * log(1.0 - x) - ln_beta)
    if x < (a + 1.0) / (a + b + 2.0):
        return min(1.0, max(0.0, front * _betacf(a, b, x) / a))
    return min(1.0, max(0.0, 1.0 - front * _betacf(b, a, 1.0 - x) / b))


def student_t_cdf(t_value: float, df: float) -> float:
    """Cumulative distribution function P(T <= t_value) for Student's t with df > 0."""
    if not (isfinite(df) and df > 0.0):
        raise ValueError("df must be strictly positive and finite")
    if not isfinite(t_value):
        if t_value == float("inf"):
            return 1.0
        if t_value == float("-inf"):
            return 0.0
        raise ValueError("t_value must not be NaN")
    if t_value == 0.0:
        return 0.5
    # Avoid overflow in t_value**2 for extreme |t|
    ratio = df / (df + t_value * t_value) if abs(t_value) < 1e150 else 0.0
    tail = 0.5 * regularized_incomplete_beta(ratio, 0.5 * df, 0.5)
    return tail if t_value < 0.0 else 1.0 - tail


def student_t_ppf(probability: float, df: float) -> float:
    """Quantile function (inverse CDF) of Student's t distribution for p in (0, 1)."""
    if not (isfinite(df) and df > 0.0):
        raise ValueError("df must be strictly positive and finite")
    if not (isfinite(probability) and 0.0 < probability < 1.0):
        raise ValueError("probability must be strictly between 0 and 1")
    if probability == 0.5:
        return 0.0
    if probability < 0.5:
        return -student_t_ppf(1.0 - probability, df)

    lo = 0.0
    hi = 1.0
    while student_t_cdf(hi, df) < probability:
        hi *= 2.0
        if not isfinite(hi):
            raise OverflowError("student_t_ppf overflowed bracket search")

    for _ in range(100):
        mid = 0.5 * (lo + hi)
        if mid == lo or mid == hi:
            break
        if student_t_cdf(mid, df) < probability:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)


__all__ = ["regularized_incomplete_beta", "student_t_cdf", "student_t_ppf"]
