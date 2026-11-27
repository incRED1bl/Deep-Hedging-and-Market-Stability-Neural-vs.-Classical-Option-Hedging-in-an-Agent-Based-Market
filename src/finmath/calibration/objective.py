"""Score the reference-market calibration grid using summary statistics from #4."""

# AI assistance: OpenAI ChatGPT. ACF motivation: https://doi.org/10.1016/0927-5398(93)90006-D
import argparse
import json
import math
import re
from pathlib import Path
from statistics import fmean

from finmath.data_release import parse_seeds

ACF_LAGS = (1, 5, 20)
METRICS = ("annualized_volatility", "p05", "p95", *(f"abs_return_acf_{lag}" for lag in ACF_LAGS))


def _moments(snapshot: dict, count: int) -> dict[str, float]:
    if count <= max(ACF_LAGS) or snapshot["return_count"] != count:
        raise ValueError(f"each summary must contain the target sample length ({count} returns)")
    tails = snapshot["log_return_quantiles"]
    acf = snapshot["acf_absolute_log_returns"]
    values = {
        "annualized_volatility": snapshot["annualized_volatility"],
        "p05": tails["p05"],
        "p95": tails["p95"],
        **{f"abs_return_acf_{lag}": acf[str(lag)] for lag in ACF_LAGS},
    }
    for metric, value in values.items():
        if type(value) not in (int, float) or not math.isfinite(value):
            raise ValueError(f"missing or non-finite {metric}")
    if values["annualized_volatility"] < 0 or values["p05"] > values["p95"]:
        raise ValueError("invalid volatility or quantile order")
    if any(abs(values[f"abs_return_acf_{lag}"]) > 1 for lag in ACF_LAGS):
        raise ValueError("ACF must be between -1 and 1")
    return values


def evaluate(target: dict, simulations: dict[str, dict], seeds: list[str]) -> dict:
    parse_seeds(seeds, source="simulation seeds")
    if not isinstance(simulations, dict) or simulations.keys() != set(seeds):
        raise ValueError("candidate must contain one summary per reference-market seed")
    count = target["return_count"]
    if type(count) is not int:
        raise ValueError("target return_count must be an integer")
    actual = _moments(target, count)
    daily_scale = target["daily_volatility"]
    annual_scale = actual["annualized_volatility"]
    if (
        type(daily_scale) not in (float, int)
        or not math.isfinite(daily_scale)
        or daily_scale <= 0
        or annual_scale <= 0
    ):
        raise ValueError("target volatility scales must be positive and finite")
    each = []
    hashes = set()
    for seed in seeds:
        summary = simulations[seed]
        digest = summary.get("config_sha256")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError(f"seed {seed}: expected the base configuration SHA256")
        hashes.add(digest)
        each.append(_moments(summary, count))
    if len(hashes) != 1:
        raise ValueError("candidate used different base configurations across seeds")
    simulated = {metric: fmean(row[metric] for row in each) for metric in METRICS}
    delta = {metric: simulated[metric] - actual[metric] for metric in METRICS}
    blocks = {
        "volatility": (delta["annualized_volatility"] / annual_scale) ** 2,
        "tails": fmean((delta[metric] / daily_scale) ** 2 for metric in ("p05", "p95")),
        "persistence": fmean(delta[f"abs_return_acf_{lag}"] ** 2 for lag in ACF_LAGS),
    }
    return {
        "config_sha256": hashes.pop(),
        "loss": fmean(blocks.values()),
        "loss_blocks": blocks,
        "target": actual,
        "simulated_mean": simulated,
        "simulated_minus_target": delta,
        "simulated_seed_count": len(each),
        "return_count_per_seed": count,
    }


def rank_candidates(meta: dict, input_grid: dict) -> dict:
    # The holdout is only evaluated in a separate command after selecting a candidate.
    target = meta["splits"]["calibration"]
    parse_seeds(input_grid["seeds"], source="calibration seeds")
    if not input_grid["candidates"]:
        raise ValueError("expected at least one candidate")
    scores, failures = [], {}
    for name, simulations in input_grid["candidates"].items():
        try:
            score = evaluate(target, simulations, input_grid["seeds"])
        except (KeyError, TypeError, ValueError, ArithmeticError) as error:
            failures[name] = str(error)
        else:
            scores.append({"candidate": name, **score})
    scores.sort(key=lambda score: (score["loss"], score["candidate"]))
    return {
        "selected": scores[0]["candidate"] if scores else None,
        "selected_config_sha256": scores[0]["config_sha256"] if scores else None,
        "reference_sha256": meta["source"]["raw_sha256"],
        "seeds": input_grid["seeds"],
        "scores": scores,
        "failures": failures,
    }


def evaluate_holdout(meta: dict, results: dict, selection: dict) -> dict:
    raw_sha256 = meta["source"]["raw_sha256"]
    if raw_sha256 != selection["reference_sha256"]:
        raise ValueError("holdout and calibration must come from the same source snapshot")
    candidate = results["candidate"]
    if not candidate or candidate != selection["selected"]:
        raise ValueError("holdout requires the selected calibration candidate")
    fit_seeds = parse_seeds(selection["seeds"], source="calibration seeds")
    holdout_seeds = parse_seeds(results["seeds"], source="holdout seeds")
    if set(holdout_seeds).intersection(fit_seeds):
        raise ValueError("holdout and calibration simulation seeds overlap")
    score = evaluate(meta["splits"]["holdout"], results["simulations"], results["seeds"])
    if score["config_sha256"] != selection["selected_config_sha256"]:
        raise ValueError("holdout base configuration SHA256 differs from the selected candidate")
    return {
        "candidate": candidate,
        "reference_sha256": raw_sha256,
        "seeds": results["seeds"],
        **score,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    for name in ("rank", "holdout"):
        action = actions.add_parser(name)
        action.add_argument("reference", type=Path, help="FRED metadata.json from #4")
        action.add_argument("results", type=Path, help="simulation summaries by seed")
        if name == "holdout":
            action.add_argument("selection", type=Path, help="saved rank report")
    args = parser.parse_args()
    try:
        meta = json.loads(args.reference.read_text(encoding="utf-8"))
        results = json.loads(args.results.read_text(encoding="utf-8"))
        if args.action == "rank":
            report = rank_candidates(meta, results)
        else:
            selection = json.loads(args.selection.read_text(encoding="utf-8"))
            report = evaluate_holdout(meta, results, selection)
        print(json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False))
    except (KeyError, TypeError, ValueError, OSError, ArithmeticError) as error:
        parser.exit(1, f"Calibration scoring failed: {error}\n")
    if args.action == "rank" and report["selected"] is None:
        parser.exit(1, "No candidate has complete, valid calibration summaries.\n")


if __name__ == "__main__":
    main()
