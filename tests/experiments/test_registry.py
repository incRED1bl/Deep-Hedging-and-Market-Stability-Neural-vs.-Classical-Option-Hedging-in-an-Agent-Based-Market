# AI assistance: OpenAI ChatGPT. All seeds and outcomes in this module are fixtures.
import json
import sqlite3
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from pathlib import Path
from threading import Lock

import pytest

from finmath.experiments.registry import Outcome, Run, fixture_plan, initialize, run_pending

SEEDS = list(range(1000, 1050))
CONFIG = {"population": 200, "burn_in_days": 500, "observed_days": 2000}


def init(db: Path, *, commit: str = "a" * 40) -> None:
    initialize(
        db, seeds=SEEDS, commit=commit, config=CONFIG, data_hash="b" * 64, model_hash="c" * 64
    )


def success(_: Run) -> Outcome:
    return Outcome(
        {"annualized_realized_volatility": 0.2, "executed_shares_per_day": 0.0, "net_pnl": -20.0},
        [{"traded_lots": 0}],
    )


def test_unique_matrix_and_fixture_seed_boundary() -> None:
    runs = list(fixture_plan(SEEDS))
    assert len({(run.policy, run.cost_bps, run.seed) for run in runs}) == 1000
    assert {run.policy for run in runs} == {"bs", "leland", "band", "dl"}
    assert {run.cost_bps for run in runs} == {0, 5, 10, 20, 40}
    assert {run.seed for run in runs if run.policy == "bs"} == set(SEEDS)
    for bad_seeds in (SEEDS[:-1], SEEDS[:-1] + [SEEDS[0]], SEEDS[:-1] + [20_000]):
        with pytest.raises(ValueError, match="fixture range"):
            list(fixture_plan(bad_seeds))


def test_resume_records_failures_and_keeps_economic_losses(tmp_path: Path) -> None:
    db = tmp_path / "runs.sqlite"
    init(db)

    def fail_once(run: Run) -> Outcome:
        if run.ordinal == 9:
            raise RuntimeError("fixture journal missing")
        return success(run)

    assert run_pending(db, fail_once) == {"done": 999, "failed": 1}
    init(db)
    with closing(sqlite3.connect(db)) as connection:
        assert connection.execute("SELECT error FROM runs WHERE ordinal=9").fetchone() == (
            "RuntimeError: fixture journal missing",
        )
        persisted = connection.execute("SELECT metrics_json FROM runs WHERE ordinal=0").fetchone()
        assert json.loads(persisted[0])["net_pnl"] == -20.0
    assert run_pending(db, success) == {"done": 999, "failed": 1}
    assert run_pending(db, success, retry_failed=True) == {"done": 1000}
    with closing(sqlite3.connect(db)) as connection:
        assert connection.execute("SELECT SUM(attempts) FROM runs").fetchone() == (1001,)
        assert connection.execute("SELECT ordinal, attempt, reason FROM run_errors").fetchall() == [
            (9, 1, "RuntimeError: fixture journal missing")
        ]
    with pytest.raises(ValueError, match="provenance differs"):
        init(db, commit="f" * 40)


def test_concurrent_resumes_do_not_repeat_successes(tmp_path: Path) -> None:
    db = tmp_path / "parallel.sqlite"
    init(db)
    seen: set[int] = set()
    guard = Lock()

    def record(run: Run) -> Outcome:
        with guard:
            assert run.ordinal not in seen
            seen.add(run.ordinal)
        return success(run)

    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(run_pending, db, record) for _ in range(2)]
        assert futures[-1].result() == {"done": 1000}
        assert futures[0].result() == {"done": 1000}
    assert len(seen) == 1000
    with closing(sqlite3.connect(db)) as connection:
        assert connection.execute("SELECT SUM(attempts) FROM runs").fetchone() == (1000,)


def test_initialize_rejects_float_seeds_before_creating_db(tmp_path: Path) -> None:
    db = tmp_path / "invalid.sqlite"
    with pytest.raises(ValueError, match="fixture range"):
        initialize(
            db,
            seeds=[1000.0, *SEEDS[1:]],
            commit="a" * 40,
            config=CONFIG,
            data_hash="b" * 64,
            model_hash="c" * 64,
        )
    assert not db.exists()


def test_limit_resumes_with_all_policies_and_costs_of_a_seed(tmp_path: Path) -> None:
    db = tmp_path / "limited.sqlite"
    init(db)
    assert run_pending(db, success, limit=3) == {"done": 3, "pending": 997}
    assert run_pending(db, success, limit=17) == {"done": 20, "pending": 980}
    with closing(sqlite3.connect(db)) as connection:
        assert connection.execute(
            "SELECT DISTINCT seed, attempts FROM runs WHERE status='done'"
        ).fetchall() == [("1000", 1)]

    with pytest.raises(ValueError, match="provenance differs"):
        initialize(
            db,
            seeds=SEEDS,
            commit="a" * 40,
            config=CONFIG,
            data_hash="b" * 64,
            model_hash="c" * 64,
            simulator={"commit": "d" * 40, "executable_sha256": "e" * 64},
        )
