"""Run the Rust integration fixture and summarize its version 1 journal."""

# AI assistance: OpenAI ChatGPT. Journal contract: docs/architecture/strategy-integration.md (#29).
import json
import math
import subprocess
from pathlib import Path
from statistics import stdev
from tempfile import TemporaryDirectory
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from finmath.experiments.registry import Run

POLICY_ARGUMENTS = {"bs": "bs", "leland": "leland", "band": "band", "dl": "neural"}


def _integer(value: object, label: str, *, minimum: int | None = None) -> int:
    if type(value) is not int or (minimum is not None and value < minimum):
        raise ValueError(f"invalid {label}: {value!r}")
    return value


def summarize_journal(directory: Path, config: dict) -> tuple[dict, list[dict]]:
    """Compute fixture metrics from observed closes, fills, and terminal accounting."""
    burn_in = _integer(config["burn_in_days"], "burn-in length", minimum=0)
    observed = _integer(config["observed_days"], "observed length", minimum=2)
    trading_days = _integer(config["trading_days_per_year"], "trading calendar", minimum=1)
    instrument = config["instrument"]
    lot_size = _integer(instrument["lot_size_micros"], "lot size", minimum=1) / 1_000_000
    cash_scale = 10 ** _integer(instrument["cash_decimal_places"], "cash precision", minimum=0)
    quote_currency = instrument["quote_currency"]
    final_step = burn_in + observed

    expected_names = {f"{step:010d}.json" for step in range(final_step + 1)}
    actual_names = {path.name for path in directory.glob("*.json")}
    if missing := expected_names - actual_names:
        raise ValueError(f"journal missing {len(missing)} steps, first is {min(missing)}")
    if extra := actual_names - expected_names:
        raise ValueError(f"journal has unexpected step {min(extra)}")

    def money_units(cash: dict, *, minimum: int | None = None) -> int:
        if cash["currency"] != quote_currency:
            raise ValueError("journal cash currency differs from the scenario")
        return _integer(cash["units"], "cash units", minimum=minimum)

    daily = []
    log_returns = []
    close = 0
    for step in range(final_step + 1):
        entry = json.loads((directory / f"{step:010d}.json").read_bytes())
        if entry["schema_version"] != 1 or entry["step"] != step:
            raise ValueError(f"journal {step}: unexpected version or step number")
        included = step > burn_in
        if entry["included_in_metrics"] is not included:
            raise ValueError(f"journal {step}: burn-in marker differs from the scenario")
        previous_close, close = close, _integer(entry["close_ticks"], "close ticks", minimum=1)
        if step == 0:
            metadata = entry["metadata"]
            for key in (
                "seed",
                "one_way_commission",
                "burn_in_days",
                "observed_days",
                "trading_days_per_year",
                "population",
                "limits",
                "call",
                "annual_funding_rate",
            ):
                if metadata[key] != config[key]:
                    raise ValueError(f"journal opening metadata: {key} differs from the scenario")
            if (
                close != instrument["initial_close_ticks"]
                or metadata["lot_size_micros"] != instrument["lot_size_micros"]
                or metadata["cash_decimal_places"] != instrument["cash_decimal_places"]
                or money_units(metadata["tick_value_per_share"])
                != instrument["tick_value_per_share"]
            ):
                raise ValueError("journal opening instrument differs from the scenario")
        if step == burn_in:
            cutoff_close = close
        if not included:
            continue
        fills = [p for p in entry["postings"] if p["type"] == "fill"]
        lots = _integer(entry["traded_lots"], "traded lots", minimum=0)
        if lots != sum(_integer(fill["lots"], "filled lots", minimum=1) for fill in fills):
            raise ValueError(f"journal {step}: traded lots differ from bilateral fills")
        fees = 2 * sum(money_units(fill["fee_per_side"], minimum=0) for fill in fills)
        log_returns.append(math.log(close / previous_close))
        daily.append({"step": step, "close_ticks": close, "traded_lots": lots, "fee_units": fees})

    pnls = entry["terminal"]["observed_hedger_pnl"]
    population = config["population"]
    first_hedger = sum(population.values()) - population["hedgers"]
    hedgers = range(first_hedger, first_hedger + population["hedgers"])
    if sorted(_integer(pnl["agent"], "hedger ID") for pnl in pnls) != list(hedgers):
        raise ValueError("journal terminal P&L must include every hedger exactly once")

    return {
        "annualized_realized_volatility": stdev(log_returns) * math.sqrt(trading_days),
        "executed_shares_per_day": sum(row["traded_lots"] for row in daily) * lot_size / observed,
        "net_pnl_observed": sum(money_units(pnl["pnl"]) for pnl in pnls) / cash_scale,
        "total_fees_observed": sum(row["fee_units"] for row in daily) / cash_scale,
        "no_trade_share": sum(row["traded_lots"] == 0 for row in daily) / observed,
        "burn_in_close_ticks": cutoff_close,
    }, daily


def run_market_fixture(
    executable: Path, model: bytes, config: dict, run: "Run"
) -> tuple[dict, list[dict]]:
    effective = {**config, "seed": run.seed, "one_way_commission": run.cost_bps / 10_000}
    with TemporaryDirectory(prefix="hedging-run-") as temp:
        directory = Path(temp)
        config_file = directory / "config.json"
        config_file.write_text(json.dumps(effective, allow_nan=False), encoding="utf-8")
        journal = directory / "journal"
        command = [
            str(executable.resolve()),
            str(config_file),
            POLICY_ARGUMENTS[run.policy],
            str(journal),
        ]
        if run.policy == "dl":
            weights = directory / "model.json"
            weights.write_bytes(model)
            command.append(str(weights))
        process = subprocess.run(command, capture_output=True, text=True, check=False, timeout=120)
        if process.returncode:
            raise RuntimeError(
                f"Rust fixture exited {process.returncode}: {process.stderr.strip()}"
            )
        return summarize_journal(journal, effective)
