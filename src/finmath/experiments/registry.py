"""Persist and resume the 4 × 5 × 50 experiment matrix on fixture seeds."""

# AI assistance: OpenAI ChatGPT. Matrix and seed conventions: research protocol #1.
import argparse
import hashlib
import json
import math
import re
import sqlite3
from collections.abc import Callable, Iterator
from contextlib import closing
from dataclasses import dataclass
from itertools import product
from pathlib import Path
from time import perf_counter

from finmath.data_release import read_seeds, sha256_file

POLICIES = ("bs", "leland", "band", "dl")
COST_BPS = (0, 5, 10, 20, 40)
FIXTURE_SEEDS = range(1000, 2000)
SCHEMA = """
CREATE TABLE IF NOT EXISTS provenance (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    manifest_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
    ordinal INTEGER PRIMARY KEY,
    policy TEXT NOT NULL,
    cost_bps INTEGER NOT NULL,
    seed TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    metrics_json TEXT,
    aggregates_json TEXT,
    elapsed_seconds REAL,
    error TEXT,
    UNIQUE (policy, cost_bps, seed)
);
CREATE TABLE IF NOT EXISTS run_errors (
    ordinal INTEGER NOT NULL REFERENCES runs(ordinal),
    attempt INTEGER NOT NULL,
    reason TEXT NOT NULL,
    PRIMARY KEY (ordinal, attempt)
);
"""


@dataclass(frozen=True)
class Run:
    ordinal: int
    policy: str
    cost_bps: int
    seed: int


@dataclass(frozen=True)
class Outcome:
    metrics: dict
    aggregates: list[dict]


def _json(value: object) -> str:
    return json.dumps(value, sort_keys=True, allow_nan=False, separators=(",", ":"))


def fixture_plan(seeds: list[int]) -> Iterator[Run]:
    if (
        len(seeds) != 50
        or len(set(seeds)) != 50
        or any(type(seed) is not int or seed not in FIXTURE_SEEDS for seed in seeds)
    ):
        raise ValueError("expected 50 distinct seeds in the fixture range 1000–1999")
    for ordinal, (policy, cost_bps, seed) in enumerate(product(POLICIES, COST_BPS, seeds)):
        yield Run(ordinal, policy, cost_bps, seed)


def initialize(
    db: Path,
    *,
    seeds: list[int],
    commit: str,
    config: dict,
    data_hash: str,
    model_hash: str,
    simulator: dict[str, str] | None = None,
) -> None:
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("expected a full Git commit SHA")
    if any(re.fullmatch(r"[0-9a-f]{64}", value) is None for value in (data_hash, model_hash)):
        raise ValueError("expected SHA256 data and model digests")
    if not isinstance(config, dict):
        raise ValueError("expected a JSON object for the scenario config")
    rows = [(run.ordinal, run.policy, run.cost_bps, str(run.seed)) for run in fixture_plan(seeds)]
    if simulator is not None and (
        set(simulator) != {"commit", "executable_sha256"}
        or re.fullmatch(r"[0-9a-f]{40}", simulator["commit"]) is None
        or re.fullmatch(r"[0-9a-f]{64}", simulator["executable_sha256"]) is None
    ):
        raise ValueError("expected the market simulator commit and executable SHA256")
    manifest = _json(
        {
            "schema_version": 1,
            "split": "fixtures",
            "commit": commit,
            "config": config,
            "data_hash": data_hash,
            "model_hash": model_hash,
            "seeds": [str(seed) for seed in seeds],
            **({"simulator": simulator} if simulator is not None else {}),
        }
    )
    db.parent.mkdir(parents=True, exist_ok=True)
    with closing(sqlite3.connect(db, timeout=30)) as connection, connection:
        connection.executescript(SCHEMA)
        connection.execute("BEGIN IMMEDIATE")
        previous = connection.execute("SELECT manifest_json FROM provenance WHERE id=1").fetchone()
        if previous is None:
            connection.execute("INSERT INTO provenance VALUES (1, ?)", (manifest,))
            connection.executemany(
                "INSERT INTO runs (ordinal, policy, cost_bps, seed) VALUES (?, ?, ?, ?)", rows
            )
        elif previous[0] != manifest:
            raise ValueError("registry provenance differs; use a separate database for this plan")
        saved = connection.execute(
            "SELECT ordinal, policy, cost_bps, seed FROM runs ORDER BY ordinal"
        ).fetchall()
        if saved != rows:
            raise ValueError("registry matrix does not match the planned combinations")


def run_pending(
    db: Path,
    execute: Callable[[Run], Outcome],
    *,
    retry_failed: bool = False,
    limit: int | None = None,
) -> dict[str, int]:
    if limit is not None and limit < 1:
        raise ValueError("run limit must be positive")
    # A writer lock spans one run, so overlapping invocations cannot execute the same cell.
    # A failed/aborted process releases the lock and leaves the cell retryable.
    with closing(sqlite3.connect(db.resolve().as_uri() + "?mode=rw", uri=True, timeout=30)) as conn:
        candidates = conn.execute(
            "SELECT ordinal FROM runs "
            "WHERE status='pending' OR (? AND status='failed') ORDER BY seed, cost_bps, policy",
            (retry_failed,),
        ).fetchall()
        attempted = 0
        for (ordinal,) in candidates:
            if attempted == limit:
                break
            with conn:
                conn.execute("BEGIN IMMEDIATE")
                policy, cost_bps, seed, status = conn.execute(
                    "SELECT policy, cost_bps, seed, status FROM runs WHERE ordinal=?", (ordinal,)
                ).fetchone()
                if status != "pending" and not (status == "failed" and retry_failed):
                    continue
                attempted += 1
                start = perf_counter()
                try:
                    result = execute(Run(ordinal, policy, cost_bps, int(seed)))
                    for key in ("annualized_realized_volatility", "executed_shares_per_day"):
                        value = result.metrics[key]
                        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                            raise ValueError(f"invalid {key}: {value!r}")
                    metrics, aggregates = _json(result.metrics), _json(result.aggregates)
                except Exception as error:
                    reason = f"{type(error).__name__}: {error}"
                    conn.execute(
                        "INSERT INTO run_errors "
                        "SELECT ordinal, attempts+1, ? FROM runs WHERE ordinal=?",
                        (reason, ordinal),
                    )
                    conn.execute(
                        """UPDATE runs SET status='failed', attempts=attempts+1,
                           elapsed_seconds=?, error=? WHERE ordinal=?""",
                        (perf_counter() - start, reason, ordinal),
                    )
                else:
                    conn.execute(
                        """UPDATE runs SET status='done', attempts=attempts+1,
                           metrics_json=?, aggregates_json=?, elapsed_seconds=?, error=NULL
                           WHERE ordinal=?""",
                        (metrics, aggregates, perf_counter() - start, ordinal),
                    )
        return dict(conn.execute("SELECT status, COUNT(*) FROM runs GROUP BY status"))


def _fixture(run: Run) -> Outcome:
    return Outcome(
        metrics={
            "annualized_realized_volatility": (run.seed % 17) / 100,
            "executed_shares_per_day": float(run.seed % 13),
            "net_pnl_observed": -float(run.cost_bps),
        },
        aggregates=[{"observed_day": 1, "executed_shares": run.seed % 13}],
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", type=Path, required=True)
    parser.add_argument("--seeds", type=Path, required=True, help="fixture seeds only")
    parser.add_argument("--config", type=Path, required=True, help="fixture scenario JSON")
    parser.add_argument("--data", type=Path, required=True, help="fixture data artifact")
    parser.add_argument("--model", type=Path, required=True, help="fixture model artifact")
    parser.add_argument("--commit", required=True)
    parser.add_argument("--retry-failed", action="store_true")
    parser.add_argument("--market-executable", type=Path, help="built Rust run_scenario fixture")
    parser.add_argument("--simulator-commit", help="Git commit used to build --market-executable")
    parser.add_argument("--limit", type=int, help="maximum runs to attempt on this invocation")
    args = parser.parse_args()
    if (args.market_executable is None) != (args.simulator_commit is None):
        parser.error("--market-executable and --simulator-commit must be supplied together")
    try:
        config = json.loads(args.config.read_text(encoding="utf-8"))
        model = args.model.read_bytes()
        execute = _fixture
        simulator = None
        if args.market_executable is not None:
            from finmath.experiments.market_fixture import run_market_fixture

            simulator = {
                "commit": args.simulator_commit,
                "executable_sha256": sha256_file(args.market_executable),
            }

            def execute(run: Run) -> Outcome:
                return Outcome(*run_market_fixture(args.market_executable, model, config, run))

        initialize(
            args.database,
            seeds=read_seeds(args.seeds),
            commit=args.commit,
            config=config,
            data_hash=sha256_file(args.data),
            model_hash=hashlib.sha256(model).hexdigest(),
            simulator=simulator,
        )
        summary = run_pending(
            args.database, execute, retry_failed=args.retry_failed, limit=args.limit
        )
    except (OSError, ValueError, sqlite3.Error) as error:
        parser.exit(1, f"Experiment registry failed: {error}\n")
    print(_json(summary))
    if summary.get("failed", 0) or (args.limit is None and summary != {"done": 1000}):
        parser.exit(1)


if __name__ == "__main__":
    main()
