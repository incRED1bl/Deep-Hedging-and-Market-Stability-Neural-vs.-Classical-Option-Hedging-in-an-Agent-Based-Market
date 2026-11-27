# AI assistance: OpenAI ChatGPT. The journal entries contain synthetic fixture data.
import json
import math
from pathlib import Path

import pytest

from finmath.experiments.market_fixture import summarize_journal

CONFIG = {
    "seed": 1000,
    "burn_in_days": 1,
    "observed_days": 2,
    "trading_days_per_year": 252,
    "one_way_commission": 0.0005,
    "population": {"noise_traders": 1, "hedgers": 1},
    "limits": {},
    "call": {},
    "annual_funding_rate": 0.0,
    "instrument": {
        "initial_close_ticks": 10_000,
        "quote_currency": "USD",
        "lot_size_micros": 10_000,
        "cash_decimal_places": 8,
        "tick_value_per_share": 1_000_000,
    },
}


def cash(units: int) -> dict:
    return {"currency": "USD", "units": units}


def write_journal(directory: Path, *, traded_lots: int = 2) -> None:
    fill = {"type": "fill", "lots": 2, "fee_per_side": cash(105_000)}
    opening = {**CONFIG, "cash_decimal_places": 8, "lot_size_micros": 10_000}
    opening["tick_value_per_share"] = cash(1_000_000)
    for step, close in enumerate((10_000, 10_500, 10_500, 10_710)):
        day = {
            "schema_version": 1,
            "step": step,
            "included_in_metrics": step > 1,
            "close_ticks": close,
            "metadata": opening if step == 0 else None,
            "traded_lots": traded_lots if step == 3 else 0,
            "postings": [fill] if step in {1, 3} else [],
            "terminal": None,
        }
        if step == 3:
            day["terminal"] = {"observed_hedger_pnl": [{"agent": 1, "pnl": cash(-300_000_000)}]}
        (directory / f"{step:010d}.json").write_text(json.dumps(day))


def test_reads_actual_volume_fees_pnl_and_burn_in(tmp_path: Path) -> None:
    write_journal(tmp_path)
    metrics, days = summarize_journal(tmp_path, CONFIG)
    assert metrics == pytest.approx(
        {
            "annualized_realized_volatility": math.log(1.02) * math.sqrt(252 / 2),
            "executed_shares_per_day": 0.01,
            "net_pnl_observed": -3,
            "total_fees_observed": 0.0021,
            "no_trade_share": 0.5,
            "burn_in_close_ticks": 10_500,
        }
    )
    assert days == [
        {"step": 2, "close_ticks": 10_500, "traded_lots": 0, "fee_units": 0},
        {"step": 3, "close_ticks": 10_710, "traded_lots": 2, "fee_units": 210_000},
    ]


def test_rejects_wrong_config_missing_steps_and_invalid_volume(tmp_path: Path) -> None:
    write_journal(tmp_path)
    with pytest.raises(ValueError, match="seed differs"):
        summarize_journal(tmp_path, {**CONFIG, "seed": 1001})
    (tmp_path / "0000000001.json").unlink()
    with pytest.raises(ValueError, match="journal missing 1 steps"):
        summarize_journal(tmp_path, CONFIG)
    write_journal(tmp_path, traded_lots=4)
    with pytest.raises(ValueError, match="traded lots differ"):
        summarize_journal(tmp_path, CONFIG)
