"""Confirmatory hypothesis tests, Holm adjustment, diagnostics, and seed-block bootstrap.

Implements research protocol #1 (docs/protocol.md, sections 5--6) for issue #13:
- H1: paired t-test on D_V(s, c0) = V(s, dl, c0) - V(s, bs, c0), H1: mean < 0.
- H2: paired t-test on D_U(s, c0) = U(s, dl, c0) - U(s, bs, c0), H1: mean < 0.
- H3: seed-level OLS slope b_s of D_V(s, c_j) on x_j = c_j / c0, tested across seeds.
- Holm adjustment across the 3 confirmatory hypotheses (alpha = 0.05).
- Marginal 95% Student-t CI, familywise Bonferroni CI, and whole-seed-block percentile bootstrap.
- Pairwise difference distribution diagnostics (skewness, kurtosis, QQ, histogram, outliers, LOO).
"""

from __future__ import annotations

import random
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from math import ceil, floor, fsum, isfinite, sqrt
from statistics import NormalDist
from typing import Any

from finmath.metrics import RunMetricsRow
from finmath.statistics.distributions import student_t_cdf, student_t_ppf

DEFAULT_ALPHA = 0.05
DEFAULT_CI_LEVEL = 0.95
DEFAULT_BOOTSTRAP_RESAMPLES = 20_000
DEFAULT_BOOTSTRAP_SEED = 71_001
DEFAULT_C0_BPS = 10
DEFAULT_COST_BPS_GRID = (0, 5, 10, 20, 40)
DEFAULT_COMPARATORS = ("bs", "leland", "band")
DEFAULT_TREATMENT = "dl"

_ALLOWED_ALTERNATIVES = frozenset({"less", "greater", "two-sided"})
_STANDARD_NORMAL = NormalDist(0.0, 1.0)


def _check_finite_sequence(values: Sequence[float], name: str, *, min_len: int = 2) -> list[float]:
    if len(values) < min_len:
        raise ValueError(f"{name} requires at least {min_len} observations")
    result: list[float] = []
    for val in values:
        if isinstance(val, bool) or not isinstance(val, int | float) or not isfinite(val):
            raise ValueError(f"{name} must contain only finite real numbers")
        result.append(float(val))
    return result


def _mean_and_sample_sd(values: Sequence[float]) -> tuple[float, float]:
    n = len(values)
    mean = fsum(values) / n
    sum_sq = fsum((v - mean) ** 2 for v in values)
    sd = sqrt(sum_sq / (n - 1))
    return mean, sd


def _quantile_sorted(sorted_values: Sequence[float], q: float) -> float:
    """Linear-interpolation quantile (R type 7 / NumPy default) on a nonempty sorted sequence."""
    n = len(sorted_values)
    if n == 1:
        return float(sorted_values[0])
    pos = q * (n - 1)
    lo = floor(pos)
    hi = ceil(pos)
    if lo == hi:
        return float(sorted_values[lo])
    return float(sorted_values[lo] + (pos - lo) * (sorted_values[hi] - sorted_values[lo]))


def _one_sample_p_value(statistic: float, df: int, alternative: str) -> float:
    if alternative == "less":
        return student_t_cdf(statistic, df)
    if alternative == "greater":
        return 1.0 - student_t_cdf(statistic, df)
    return min(1.0, 2.0 * student_t_cdf(-abs(statistic), df))


@dataclass(frozen=True)
class ConfidenceInterval:
    """Confidence interval with explicit coverage type and degeneracy flag."""

    lower: float | None
    upper: float | None
    level: float
    coverage_type: str
    method: str
    degenerate: bool = False
    note: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "lower": self.lower,
            "upper": self.upper,
            "level": self.level,
            "coverage_type": self.coverage_type,
            "method": self.method,
            "degenerate": self.degenerate,
            "note": self.note,
        }


@dataclass(frozen=True)
class HypothesisTestResult:
    """Complete summary for one confirmatory hypothesis."""

    hypothesis_id: str
    endpoint: str
    contrast: str
    alternative: str
    n_seeds: int
    df: int
    estimate: float
    sd: float
    se: float
    statistic: float | None
    p_raw: float
    p_adjusted: float | None
    alpha: float
    rejected: bool | None
    estimable: bool
    degenerate_reason: str | None
    relative_change: float | None
    standardized_dz: float | None
    t_ci: ConfidenceInterval
    bonferroni_t_ci: ConfidenceInterval
    bootstrap_ci: ConfidenceInterval | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "hypothesis_id": self.hypothesis_id,
            "endpoint": self.endpoint,
            "contrast": self.contrast,
            "alternative": self.alternative,
            "n_seeds": self.n_seeds,
            "df": self.df,
            "estimate": self.estimate,
            "sd": self.sd,
            "se": self.se,
            "statistic": self.statistic,
            "p_raw": self.p_raw,
            "p_adjusted": self.p_adjusted,
            "alpha": self.alpha,
            "rejected": self.rejected,
            "estimable": self.estimable,
            "degenerate_reason": self.degenerate_reason,
            "relative_change": self.relative_change,
            "standardized_dz": self.standardized_dz,
            "t_ci": self.t_ci.to_dict(),
            "bonferroni_t_ci": self.bonferroni_t_ci.to_dict(),
            "bootstrap_ci": None if self.bootstrap_ci is None else self.bootstrap_ci.to_dict(),
        }


@dataclass(frozen=True)
class DistributionDiagnostics:
    """Distribution checks for seed-level paired differences or slopes (protocol section 6.6)."""

    name: str
    n_seeds: int
    mean: float
    sd: float
    min_value: float
    q25: float
    median: float
    q75: float
    max_value: float
    iqr: float
    skewness: float | None
    excess_kurtosis: float | None
    min_seed: int
    max_seed: int
    outlier_seeds: tuple[int, ...]
    leave_one_out_min: float
    leave_one_out_max: float
    leave_one_out_max_abs_shift: float
    most_influential_seed: int
    sign_flips_on_leave_one_out: bool
    qq_points: tuple[tuple[float, float], ...]
    histogram_edges: tuple[float, ...]
    histogram_counts: tuple[int, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "n_seeds": self.n_seeds,
            "mean": self.mean,
            "sd": self.sd,
            "min_value": self.min_value,
            "q25": self.q25,
            "median": self.median,
            "q75": self.q75,
            "max_value": self.max_value,
            "iqr": self.iqr,
            "skewness": self.skewness,
            "excess_kurtosis": self.excess_kurtosis,
            "min_seed": str(self.min_seed),
            "max_seed": str(self.max_seed),
            "outlier_seeds": [str(s) for s in self.outlier_seeds],
            "leave_one_out_min": self.leave_one_out_min,
            "leave_one_out_max": self.leave_one_out_max,
            "leave_one_out_max_abs_shift": self.leave_one_out_max_abs_shift,
            "most_influential_seed": str(self.most_influential_seed),
            "sign_flips_on_leave_one_out": self.sign_flips_on_leave_one_out,
            "qq_points": [[q, v] for q, v in self.qq_points],
            "histogram_edges": list(self.histogram_edges),
            "histogram_counts": list(self.histogram_counts),
        }


@dataclass(frozen=True)
class DescriptiveContrastRow:
    """Marginal descriptive paired contrast across cost levels and comparators."""

    metric: str
    treatment: str
    comparator: str
    cost_bps: int
    n_seeds: int
    estimate: float
    relative_change: float | None
    bootstrap_ci: ConfidenceInterval

    def to_dict(self) -> dict[str, Any]:
        return {
            "metric": self.metric,
            "treatment": self.treatment,
            "comparator": self.comparator,
            "cost_bps": self.cost_bps,
            "n_seeds": self.n_seeds,
            "estimate": self.estimate,
            "relative_change": self.relative_change,
            "bootstrap_ci": self.bootstrap_ci.to_dict(),
        }


@dataclass(frozen=True)
class ProtocolAnalysisResult:
    """Full confirmatory and descriptive statistical output for a protocol matrix."""

    n_seeds: int
    seeds: tuple[int, ...]
    alpha: float
    ci_level: float
    multiple_testing: str
    bootstrap_unit: str
    bootstrap_resamples: int
    bootstrap_seed: int
    hypotheses: tuple[HypothesisTestResult, ...]
    diagnostics: dict[str, DistributionDiagnostics]
    ci_disagreement: dict[str, bool]
    descriptive_contrasts: tuple[DescriptiveContrastRow, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "n_seeds": self.n_seeds,
            "seeds": [str(s) for s in self.seeds],
            "alpha": self.alpha,
            "ci_level": self.ci_level,
            "multiple_testing": self.multiple_testing,
            "bootstrap_unit": self.bootstrap_unit,
            "bootstrap_resamples": self.bootstrap_resamples,
            "bootstrap_seed": self.bootstrap_seed,
            "hypotheses": [h.to_dict() for h in self.hypotheses],
            "diagnostics": {k: v.to_dict() for k, v in self.diagnostics.items()},
            "ci_disagreement": dict(self.ci_disagreement),
            "descriptive_contrasts": [row.to_dict() for row in self.descriptive_contrasts],
        }


def holm_adjust(p_values: Sequence[float]) -> tuple[float, ...]:
    """Holm step-down familywise error rate adjustment, preserving original order.

    For sorted p_(1) <= ... <= p_(m), p_adj(k) = min(1, max_{i <= k} (m - i + 1) * p_(i)).
    """
    if not p_values:
        raise ValueError("p_values must be nonempty")
    validated: list[float] = []
    for p in p_values:
        if isinstance(p, bool) or not isinstance(p, int | float) or not isfinite(p):
            raise ValueError("p_values must be finite numbers in [0, 1]")
        if not 0.0 <= p <= 1.0:
            raise ValueError("p_values must lie in [0, 1]")
        validated.append(float(p))

    m = len(validated)
    order = sorted(range(m), key=lambda idx: validated[idx])
    adjusted = [0.0] * m
    running_max = 0.0
    for rank, idx in enumerate(order):
        candidate = (m - rank) * validated[idx]
        if candidate > running_max:
            running_max = candidate
        adjusted[idx] = min(1.0, running_max)
    return tuple(adjusted)


def _one_sample_t_summary(
    values: Sequence[float],
    *,
    hypothesis_id: str,
    endpoint: str,
    contrast: str,
    alternative: str = "less",
    alpha: float = DEFAULT_ALPHA,
    ci_level: float = DEFAULT_CI_LEVEL,
    family_size: int = 3,
    comparator_mean: float | None = None,
) -> HypothesisTestResult:
    if alternative not in _ALLOWED_ALTERNATIVES:
        raise ValueError(f"alternative must be one of {sorted(_ALLOWED_ALTERNATIVES)}")
    if not (isfinite(alpha) and 0.0 < alpha < 1.0):
        raise ValueError("alpha must be in (0, 1)")
    if not (isfinite(ci_level) and 0.0 < ci_level < 1.0):
        raise ValueError("ci_level must be in (0, 1)")
    if type(family_size) is not int or family_size < 1:
        raise ValueError("family_size must be a positive integer")

    zs = _check_finite_sequence(values, "seed-level observations", min_len=2)
    n = len(zs)
    df = n - 1
    estimate, sd = _mean_and_sample_sd(zs)
    se = sd / sqrt(n)

    rel_change: float | None = None
    if comparator_mean is not None and comparator_mean != 0.0:
        rel_change = estimate / comparator_mean

    bonferroni_level = 1.0 - alpha / family_size

    if sd == 0.0:
        if estimate == 0.0:
            statistic: float | None = 0.0
            p_raw = 1.0
            estimable = True
            reason: str | None = "zero_variance_zero_effect"
            note = "zero variance; t-CI is undefined"
        else:
            statistic = None
            p_raw = 1.0
            estimable = False
            reason = "zero_variance_nonzero_effect"
            note = "constant nonzero effect across seeds; confirmatory t-test is non-estimable"
        t_ci = ConfidenceInterval(
            lower=None,
            upper=None,
            level=ci_level,
            coverage_type="marginal",
            method="student-t",
            degenerate=True,
            note=note,
        )
        bonf_ci = ConfidenceInterval(
            lower=None,
            upper=None,
            level=bonferroni_level,
            coverage_type="familywise-bonferroni",
            method="student-t",
            degenerate=True,
            note=note,
        )
        dz: float | None = None
    else:
        statistic = estimate / se
        p_raw = _one_sample_p_value(statistic, df, alternative)
        estimable = True
        reason = None
        dz = estimate / sd

        t_crit = student_t_ppf(0.5 + 0.5 * ci_level, df)
        half = t_crit * se
        t_ci = ConfidenceInterval(
            lower=estimate - half,
            upper=estimate + half,
            level=ci_level,
            coverage_type="marginal",
            method="student-t",
            degenerate=False,
        )

        t_crit_bonf = student_t_ppf(0.5 + 0.5 * bonferroni_level, df)
        half_bonf = t_crit_bonf * se
        bonf_ci = ConfidenceInterval(
            lower=estimate - half_bonf,
            upper=estimate + half_bonf,
            level=bonferroni_level,
            coverage_type="familywise-bonferroni",
            method="student-t",
            degenerate=False,
        )

    return HypothesisTestResult(
        hypothesis_id=hypothesis_id,
        endpoint=endpoint,
        contrast=contrast,
        alternative=alternative,
        n_seeds=n,
        df=df,
        estimate=estimate,
        sd=sd,
        se=se,
        statistic=statistic,
        p_raw=p_raw,
        p_adjusted=None,
        alpha=alpha,
        rejected=None,
        estimable=estimable,
        degenerate_reason=reason,
        relative_change=rel_change,
        standardized_dz=dz,
        t_ci=t_ci,
        bonferroni_t_ci=bonf_ci,
    )


def paired_t_test(
    treatment_values: Sequence[float],
    comparator_values: Sequence[float],
    *,
    hypothesis_id: str = "H1",
    endpoint: str = "endpoint",
    contrast: str = "treatment-minus-comparator",
    alternative: str = "less",
    alpha: float = DEFAULT_ALPHA,
    ci_level: float = DEFAULT_CI_LEVEL,
    family_size: int = 3,
) -> HypothesisTestResult:
    """Paired seed-level t-test on (treatment_s - comparator_s)."""
    treat = _check_finite_sequence(treatment_values, "treatment_values", min_len=2)
    comp = _check_finite_sequence(comparator_values, "comparator_values", min_len=2)
    if len(treat) != len(comp):
        raise ValueError("treatment_values and comparator_values must have identical length")
    diffs = [t - c for t, c in zip(treat, comp, strict=True)]
    comp_mean = fsum(comp) / len(comp)
    return _one_sample_t_summary(
        diffs,
        hypothesis_id=hypothesis_id,
        endpoint=endpoint,
        contrast=contrast,
        alternative=alternative,
        alpha=alpha,
        ci_level=ci_level,
        family_size=family_size,
        comparator_mean=comp_mean,
    )


def seed_ols_slopes(
    differences_by_seed: Sequence[Sequence[float]],
    cost_multipliers: Sequence[float],
) -> tuple[float, ...]:
    """Compute within-seed OLS slope b_s = sum((x_j - mean_x)*d_sj) / sum((x_j - mean_x)^2)."""
    xs = _check_finite_sequence(cost_multipliers, "cost_multipliers", min_len=2)
    mean_x = fsum(xs) / len(xs)
    centered_x = [x - mean_x for x in xs]
    sxx = fsum(cx * cx for cx in centered_x)
    if sxx == 0.0:
        raise ValueError("cost_multipliers must contain at least two distinct values")

    if len(differences_by_seed) < 2:
        raise ValueError("at least 2 market seeds are required for slope inference")

    slopes: list[float] = []
    for seed_diffs in differences_by_seed:
        ds = _check_finite_sequence(seed_diffs, "seed differences across costs", min_len=len(xs))
        if len(ds) != len(xs):
            raise ValueError("each seed must supply one difference per cost level")
        sxy = fsum(cx * d for cx, d in zip(centered_x, ds, strict=True))
        slopes.append(sxy / sxx)
    return tuple(slopes)


def seed_slope_test(
    differences_by_seed: Sequence[Sequence[float]],
    cost_multipliers: Sequence[float],
    *,
    hypothesis_id: str = "H3",
    endpoint: str = "volatility_gap_slope_per_c0",
    contrast: str = "mean-within-seed-ols-slope",
    alternative: str = "less",
    alpha: float = DEFAULT_ALPHA,
    ci_level: float = DEFAULT_CI_LEVEL,
    family_size: int = 3,
) -> HypothesisTestResult:
    """Seed-level regression test for trend across cost levels (protocol section 5, H3)."""
    slopes = seed_ols_slopes(differences_by_seed, cost_multipliers)
    return _one_sample_t_summary(
        slopes,
        hypothesis_id=hypothesis_id,
        endpoint=endpoint,
        contrast=contrast,
        alternative=alternative,
        alpha=alpha,
        ci_level=ci_level,
        family_size=family_size,
        comparator_mean=None,
    )


def distribution_diagnostics(
    values: Sequence[float],
    seeds: Sequence[int],
    *,
    name: str,
    bins: int = 10,
) -> DistributionDiagnostics:
    """Inspect distribution of seed-level paired differences or slopes."""
    zs = _check_finite_sequence(values, name, min_len=2)
    if len(seeds) != len(zs):
        raise ValueError("seeds and values must have identical length")
    if type(bins) is not int or bins < 1:
        raise ValueError("bins must be a positive integer")

    n = len(zs)
    mean, sd = _mean_and_sample_sd(zs)
    sorted_vals = sorted(zs)
    min_val = sorted_vals[0]
    max_val = sorted_vals[-1]
    q25 = _quantile_sorted(sorted_vals, 0.25)
    median = _quantile_sorted(sorted_vals, 0.50)
    q75 = _quantile_sorted(sorted_vals, 0.75)
    iqr = q75 - q25

    skewness: float | None = None
    excess_kurtosis: float | None = None
    if sd > 0.0:
        if n >= 3:
            m3 = fsum(((z - mean) / sd) ** 3 for z in zs)
            skewness = (n / ((n - 1) * (n - 2))) * m3
        if n >= 4:
            m4 = fsum(((z - mean) / sd) ** 4 for z in zs)
            term1 = (n * (n + 1)) / ((n - 1) * (n - 2) * (n - 3)) * m4
            term2 = (3.0 * (n - 1) ** 2) / ((n - 2) * (n - 3))
            excess_kurtosis = term1 - term2

    min_idx = min(range(n), key=lambda i: zs[i])
    max_idx = max(range(n), key=lambda i: zs[i])
    lo_fence = q25 - 1.5 * iqr
    hi_fence = q75 + 1.5 * iqr
    outliers = tuple(seeds[i] for i in range(n) if zs[i] < lo_fence or zs[i] > hi_fence)

    total = fsum(zs)
    loo_means = [(total - zs[i]) / (n - 1) for i in range(n)]
    loo_min = min(loo_means)
    loo_max = max(loo_means)
    shifts = [abs(m - mean) for m in loo_means]
    influential_idx = max(range(n), key=lambda i: shifts[i])
    sign_flips = (
        (mean < 0.0 and loo_max >= 0.0)
        or (mean > 0.0 and loo_min <= 0.0)
        or (mean == 0.0 and (loo_min < 0.0 or loo_max > 0.0))
    )

    # Blom plotting positions for normal QQ plot
    qq_points = tuple(
        (_STANDARD_NORMAL.inv_cdf((i + 1 - 0.375) / (n + 0.25)), sorted_vals[i]) for i in range(n)
    )

    if min_val == max_val:
        hist_edges = (min_val, max_val)
        hist_counts = (n,)
    else:
        width = (max_val - min_val) / bins
        edges = [min_val + k * width for k in range(bins)] + [max_val]
        counts = [0] * bins
        for z in zs:
            bucket = min(bins - 1, int((z - min_val) / width))
            counts[bucket] += 1
        hist_edges = tuple(edges)
        hist_counts = tuple(counts)

    return DistributionDiagnostics(
        name=name,
        n_seeds=n,
        mean=mean,
        sd=sd,
        min_value=min_val,
        q25=q25,
        median=median,
        q75=q75,
        max_value=max_val,
        iqr=iqr,
        skewness=skewness,
        excess_kurtosis=excess_kurtosis,
        min_seed=seeds[min_idx],
        max_seed=seeds[max_idx],
        outlier_seeds=outliers,
        leave_one_out_min=loo_min,
        leave_one_out_max=loo_max,
        leave_one_out_max_abs_shift=shifts[influential_idx],
        most_influential_seed=seeds[influential_idx],
        sign_flips_on_leave_one_out=sign_flips,
        qq_points=qq_points,
        histogram_edges=hist_edges,
        histogram_counts=hist_counts,
    )


def seed_block_bootstrap(
    seed_scalars: Mapping[str, Sequence[float]],
    *,
    resamples: int = DEFAULT_BOOTSTRAP_RESAMPLES,
    bootstrap_seed: int = DEFAULT_BOOTSTRAP_SEED,
    ci_level: float = DEFAULT_CI_LEVEL,
) -> dict[str, ConfidenceInterval]:
    """Whole-market-seed-block percentile bootstrap across multiple seed-level estimands.

    All keys share the exact same resampled seed-block indices per replicate, preserving
    cross-policy and cross-cost dependence within each market seed.
    """
    if not seed_scalars:
        raise ValueError("seed_scalars must be nonempty")
    if type(resamples) is not int or resamples < 1:
        raise ValueError("resamples must be a positive integer")
    if type(bootstrap_seed) is not int:
        raise ValueError("bootstrap_seed must be an integer")
    if not (isfinite(ci_level) and 0.0 < ci_level < 1.0):
        raise ValueError("ci_level must be in (0, 1)")

    cleaned: dict[str, list[float]] = {}
    n_seeds: int | None = None
    for key, vals in seed_scalars.items():
        arr = _check_finite_sequence(vals, f"seed_scalars[{key!r}]", min_len=2)
        if n_seeds is None:
            n_seeds = len(arr)
        elif len(arr) != n_seeds:
            raise ValueError("all seed_scalars entries must share the same number of seeds")
        cleaned[key] = arr

    assert n_seeds is not None
    degenerate_keys = {k for k, arr in cleaned.items() if min(arr) == max(arr)}
    active_keys = [k for k in cleaned if k not in degenerate_keys]

    draws: dict[str, list[float]] = {k: [] for k in active_keys}
    if active_keys:
        rng = random.Random(bootstrap_seed)
        seed_indices = range(n_seeds)
        inv_n = 1.0 / n_seeds
        for _ in range(resamples):
            chosen = rng.choices(seed_indices, k=n_seeds)
            for key in active_keys:
                arr = cleaned[key]
                draws[key].append(fsum(arr[i] for i in chosen) * inv_n)

    alpha_half = 0.5 * (1.0 - ci_level)
    intervals: dict[str, ConfidenceInterval] = {}
    for key, arr in cleaned.items():
        if key in degenerate_keys:
            point = arr[0]
            intervals[key] = ConfidenceInterval(
                lower=point,
                upper=point,
                level=ci_level,
                coverage_type="marginal",
                method="seed-block-percentile-bootstrap",
                degenerate=True,
                note="degenerate point bootstrap (zero seed variance); 95% coverage does not apply",
            )
        else:
            sorted_draws = sorted(draws[key])
            intervals[key] = ConfidenceInterval(
                lower=_quantile_sorted(sorted_draws, alpha_half),
                upper=_quantile_sorted(sorted_draws, 1.0 - alpha_half),
                level=ci_level,
                coverage_type="marginal",
                method="seed-block-percentile-bootstrap",
                degenerate=False,
            )
    return intervals


def _ci_covers_zero(ci: ConfidenceInterval | None) -> bool | None:
    if ci is None or ci.degenerate or ci.lower is None or ci.upper is None:
        return None
    return ci.lower <= 0.0 <= ci.upper


def analyze_protocol_matrix(
    rows: Sequence[RunMetricsRow | Mapping[str, Any]],
    *,
    c0_bps: int = DEFAULT_C0_BPS,
    cost_bps_grid: Sequence[int] = DEFAULT_COST_BPS_GRID,
    treatment: str = DEFAULT_TREATMENT,
    primary_comparator: str = "bs",
    comparators: Sequence[str] = DEFAULT_COMPARATORS,
    alpha: float = DEFAULT_ALPHA,
    ci_level: float = DEFAULT_CI_LEVEL,
    bootstrap_resamples: int = DEFAULT_BOOTSTRAP_RESAMPLES,
    bootstrap_seed: int = DEFAULT_BOOTSTRAP_SEED,
) -> ProtocolAnalysisResult:
    """Run the full confirmatory (H1/H2/H3 + Holm) and descriptive analysis on run rows.

    Rejects duplicate keys or incomplete seed blocks (no silent complete-case filtering).
    """
    if not rows:
        raise ValueError("rows must be nonempty")
    if type(c0_bps) is not int or c0_bps <= 0:
        raise ValueError("c0_bps must be a positive integer")
    cost_grid = tuple(cost_bps_grid)
    if len(cost_grid) < 2 or len(set(cost_grid)) != len(cost_grid):
        raise ValueError("cost_bps_grid must contain at least 2 unique cost levels")
    if c0_bps not in cost_grid:
        raise ValueError("c0_bps must be included in cost_bps_grid")

    parsed: list[RunMetricsRow] = [
        row if isinstance(row, RunMetricsRow) else RunMetricsRow.from_dict(dict(row))
        for row in rows
    ]

    index: dict[tuple[str, int, int], RunMetricsRow] = {}
    all_seeds: set[int] = set()
    for row in parsed:
        key = (row.policy, row.cost_bps, row.seed)
        if key in index:
            raise ValueError(f"duplicate run row for key {key}")
        index[key] = row
        all_seeds.add(row.seed)

    seeds = tuple(sorted(all_seeds))
    n_seeds = len(seeds)
    if n_seeds < 2:
        raise ValueError("at least 2 distinct market seeds are required")

    present_policies = {row.policy for row in parsed}
    required_policies = {treatment, primary_comparator}
    available_comparators = [
        comp for comp in comparators if comp in present_policies or comp == primary_comparator
    ]
    for pol in required_policies | set(available_comparators):
        missing = [
            (pol, cost, s) for cost in cost_grid for s in seeds if (pol, cost, s) not in index
        ]
        if missing:
            raise ValueError(
                f"incomplete matrix for policy {pol!r}; missing {len(missing)} cells "
                f"(first: {missing[0]})"
            )

    # Extract confirmatory seed-level series
    dl_v_c0 = [index[(treatment, c0_bps, s)].annualized_realized_volatility for s in seeds]
    bs_v_c0 = [index[(primary_comparator, c0_bps, s)].annualized_realized_volatility for s in seeds]
    dv_c0 = [d - b for d, b in zip(dl_v_c0, bs_v_c0, strict=True)]

    dl_u_c0 = [index[(treatment, c0_bps, s)].executed_shares_per_day for s in seeds]
    bs_u_c0 = [index[(primary_comparator, c0_bps, s)].executed_shares_per_day for s in seeds]
    du_c0 = [d - b for d, b in zip(dl_u_c0, bs_u_c0, strict=True)]

    cost_multipliers = tuple(c / c0_bps for c in cost_grid)
    dv_matrix = [
        [
            index[(treatment, cost, s)].annualized_realized_volatility
            - index[(primary_comparator, cost, s)].annualized_realized_volatility
            for cost in cost_grid
        ]
        for s in seeds
    ]
    slopes = seed_ols_slopes(dv_matrix, cost_multipliers)

    h1_raw = paired_t_test(
        dl_v_c0,
        bs_v_c0,
        hypothesis_id="H1",
        endpoint="annualized_realized_volatility",
        contrast=f"{treatment}-minus-{primary_comparator}-at-c0",
        alternative="less",
        alpha=alpha,
        ci_level=ci_level,
        family_size=3,
    )
    h2_raw = paired_t_test(
        dl_u_c0,
        bs_u_c0,
        hypothesis_id="H2",
        endpoint="executed_shares_per_day",
        contrast=f"{treatment}-minus-{primary_comparator}-at-c0",
        alternative="less",
        alpha=alpha,
        ci_level=ci_level,
        family_size=3,
    )
    h3_raw = seed_slope_test(
        dv_matrix,
        cost_multipliers,
        hypothesis_id="H3",
        endpoint="volatility_gap_slope_per_c0",
        contrast="mean-within-seed-ols-slope",
        alternative="less",
        alpha=alpha,
        ci_level=ci_level,
        family_size=3,
    )

    p_adjusted = holm_adjust([h1_raw.p_raw, h2_raw.p_raw, h3_raw.p_raw])

    # Collect all seed-level scalars for a single joint whole-seed-block bootstrap
    boot_inputs: dict[str, Sequence[float]] = {
        "H1": dv_c0,
        "H2": du_c0,
        "H3": slopes,
    }
    descriptive_specs: list[tuple[str, str, int, list[float], float]] = []
    descriptive_metrics = (
        "annualized_realized_volatility",
        "executed_shares_per_day",
        "total_commission_paid",
        "net_pnl_observed",
        "hedging_error_rmse",
    )
    for comp in available_comparators:
        for cost in cost_grid:
            for metric in descriptive_metrics:
                t_vals = [getattr(index[(treatment, cost, s)], metric) for s in seeds]
                c_vals = [getattr(index[(comp, cost, s)], metric) for s in seeds]
                if any(v is None for v in t_vals) or any(v is None for v in c_vals):
                    continue
                diffs = [float(t) - float(c) for t, c in zip(t_vals, c_vals, strict=True)]
                comp_mean = fsum(float(c) for c in c_vals) / n_seeds
                boot_key = f"desc:{metric}:{comp}:{cost}"
                boot_inputs[boot_key] = diffs
                descriptive_specs.append((metric, comp, cost, diffs, comp_mean))

    boot_cis = seed_block_bootstrap(
        boot_inputs,
        resamples=bootstrap_resamples,
        bootstrap_seed=bootstrap_seed,
        ci_level=ci_level,
    )

    final_hypotheses: list[HypothesisTestResult] = []
    ci_disagreement: dict[str, bool] = {}
    for raw, p_adj in zip((h1_raw, h2_raw, h3_raw), p_adjusted, strict=True):
        b_ci = boot_cis[raw.hypothesis_id]
        rejected = bool(raw.estimable and p_adj < alpha)
        final_hypotheses.append(
            HypothesisTestResult(
                hypothesis_id=raw.hypothesis_id,
                endpoint=raw.endpoint,
                contrast=raw.contrast,
                alternative=raw.alternative,
                n_seeds=raw.n_seeds,
                df=raw.df,
                estimate=raw.estimate,
                sd=raw.sd,
                se=raw.se,
                statistic=raw.statistic,
                p_raw=raw.p_raw,
                p_adjusted=p_adj,
                alpha=raw.alpha,
                rejected=rejected,
                estimable=raw.estimable,
                degenerate_reason=raw.degenerate_reason,
                relative_change=raw.relative_change,
                standardized_dz=raw.standardized_dz,
                t_ci=raw.t_ci,
                bonferroni_t_ci=raw.bonferroni_t_ci,
                bootstrap_ci=b_ci,
            )
        )
        t_zero = _ci_covers_zero(raw.t_ci)
        b_zero = _ci_covers_zero(b_ci)
        ci_disagreement[raw.hypothesis_id] = (
            t_zero is not None and b_zero is not None and t_zero != b_zero
        )

    diagnostics = {
        "H1": distribution_diagnostics(dv_c0, seeds, name="H1: D_V(c0)"),
        "H2": distribution_diagnostics(du_c0, seeds, name="H2: D_U(c0)"),
        "H3": distribution_diagnostics(slopes, seeds, name="H3: b_s"),
    }

    descriptive_rows = tuple(
        DescriptiveContrastRow(
            metric=metric,
            treatment=treatment,
            comparator=comp,
            cost_bps=cost,
            n_seeds=n_seeds,
            estimate=fsum(diffs) / n_seeds,
            relative_change=(fsum(diffs) / n_seeds) / comp_mean if comp_mean != 0.0 else None,
            bootstrap_ci=boot_cis[f"desc:{metric}:{comp}:{cost}"],
        )
        for metric, comp, cost, diffs, comp_mean in descriptive_specs
    )

    return ProtocolAnalysisResult(
        n_seeds=n_seeds,
        seeds=seeds,
        alpha=alpha,
        ci_level=ci_level,
        multiple_testing="holm-3-hypotheses",
        bootstrap_unit="whole-market-seed-block",
        bootstrap_resamples=bootstrap_resamples,
        bootstrap_seed=bootstrap_seed,
        hypotheses=tuple(final_hypotheses),
        diagnostics=diagnostics,
        ci_disagreement=ci_disagreement,
        descriptive_contrasts=descriptive_rows,
    )


__all__ = [
    "DEFAULT_ALPHA",
    "DEFAULT_BOOTSTRAP_RESAMPLES",
    "DEFAULT_BOOTSTRAP_SEED",
    "DEFAULT_C0_BPS",
    "DEFAULT_CI_LEVEL",
    "DEFAULT_COMPARATORS",
    "DEFAULT_COST_BPS_GRID",
    "DEFAULT_TREATMENT",
    "ConfidenceInterval",
    "DescriptiveContrastRow",
    "DistributionDiagnostics",
    "HypothesisTestResult",
    "ProtocolAnalysisResult",
    "analyze_protocol_matrix",
    "distribution_diagnostics",
    "holm_adjust",
    "paired_t_test",
    "seed_block_bootstrap",
    "seed_ols_slopes",
    "seed_slope_test",
]
