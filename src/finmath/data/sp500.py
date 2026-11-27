"""Prepare FRED's 2017–2025 S&P 500 close series for calibration and holdout."""

# AI assistance: OpenAI ChatGPT. Source definitions: https://fred.stlouisfed.org/series/SP500
import argparse
import csv
import hashlib
import io
import json
import math
from collections import Counter
from datetime import date, timedelta
from pathlib import Path
from statistics import fmean, quantiles, stdev
from tempfile import TemporaryDirectory
from urllib.request import urlopen

SERIES_URL = "https://fred.stlouisfed.org/series/SP500"
START = date(2017, 1, 1)
SPLIT = date(2023, 1, 1)
END = date(2025, 12, 31)
CSV_URL = f"https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd={START}&coed={END}"
TRADING_DAYS_PER_YEAR = 252
ACF_LAGS = (1, 2, 3, 4, 5, 10, 20)


def read_observations(raw: bytes) -> dict[date, float | None]:
    """Validate raw rows and preserve FRED's explicit missing markers."""
    reader = csv.DictReader(io.StringIO(raw.decode("utf-8-sig"), newline=""), strict=True)
    if reader.fieldnames != ["observation_date", "SP500"]:
        raise ValueError("expected FRED CSV columns observation_date,SP500")
    prices: dict[date, float | None] = {}
    for row in reader:
        if None in row or row["SP500"] is None:
            raise ValueError(f"line {reader.line_num}: expected exactly two fields")
        day = date.fromisoformat(row["observation_date"])
        if not START <= day <= END or day.isoformat() != row["observation_date"]:
            raise ValueError(f"line {reader.line_num}: unexpected observation date {day}")
        if day in prices:
            raise ValueError(f"duplicate date {day}")
        text = row["SP500"].strip()
        if text in {"", "."}:
            prices[day] = None
            continue
        price = float(text)
        if day.weekday() >= 5 or not math.isfinite(price) or price <= 0:
            raise ValueError(f"invalid closing price on {day}: {text!r}")
        prices[day] = price
    return prices


def coverage(observations: dict[date, float | None]) -> dict:
    closes_by_year = Counter(day.year for day, price in observations.items() if price is not None)
    if set(closes_by_year) != set(range(START.year, END.year + 1)):
        raise ValueError("FRED response does not cover every year from 2017 through 2025")
    days = (START + timedelta(days=offset) for offset in range((END - START).days + 1))
    absent_weekdays = [
        day.isoformat() for day in days if day.weekday() < 5 and day not in observations
    ]
    return {
        "csv_rows": len(observations),
        "closes_by_year": dict(sorted(closes_by_year.items())),
        "missing_markers_by_year": dict(
            sorted(
                Counter(day.year for day, price in observations.items() if price is None).items()
            )
        ),
        "weekdays_without_csv_row": absent_weekdays,
    }


def autocorrelations(values: list[float]) -> dict[int, float | None]:
    """Use the full-sample mean and variance for each lag in trading time."""
    mean = fmean(values)
    centered = [value - mean for value in values]
    denominator = math.fsum(value**2 for value in centered)
    return {
        lag: math.fsum(a * b for a, b in zip(centered[lag:], centered[:-lag], strict=True))
        / denominator
        if lag < len(values) and denominator > 0
        else None
        for lag in ACF_LAGS
    }


def describe_returns(returns: list[float]) -> dict:
    if len(returns) < 2:
        raise ValueError("each split needs at least two returns")
    daily_vol = stdev(returns)
    percentiles = quantiles(returns, n=100, method="inclusive")
    return {
        "return_count": len(returns),
        "mean_daily_log_return": fmean(returns),
        "daily_volatility": daily_vol,
        "annualized_volatility": daily_vol * math.sqrt(TRADING_DAYS_PER_YEAR),
        "log_return_quantiles": {
            "min": min(returns),
            **{f"p{p:02}": percentiles[p - 1] for p in (1, 5, 25, 50, 75, 95, 99)},
            "max": max(returns),
        },
        "acf_log_returns": autocorrelations(returns),
        "acf_absolute_log_returns": autocorrelations([abs(value) for value in returns]),
    }


def prepare(raw: bytes, output: Path, expected_sha256: str | None = None) -> dict:
    raw_sha256 = hashlib.sha256(raw).hexdigest()
    if expected_sha256 is not None and expected_sha256 != raw_sha256:
        raise ValueError(f"source SHA256 mismatch: got {raw_sha256}")
    observations = read_observations(raw)
    audit = coverage(observations)
    splits: dict[str, list[tuple[date, float, float | None]]] = {"calibration": [], "holdout": []}
    previous_price = None
    for day, price in sorted(observations.items()):
        if price is None:
            continue
        split = "calibration" if day < SPLIT else "holdout"
        log_return = (
            math.log(price) - math.log(previous_price) if previous_price is not None else None
        )
        splits[split].append((day, price, log_return))
        previous_price = price

    split_metadata = {}
    for name, rows in splits.items():
        split_metadata[name] = {
            "file": f"{name}.csv",
            "first_close": rows[0][0].isoformat(),
            "last_close": rows[-1][0].isoformat(),
            "close_count": len(rows),
            **describe_returns([value for _, _, value in rows if value is not None]),
        }
    metadata = {
        "schema_version": 1,
        "source": {
            "series": "SP500",
            "provider": "S&P Dow Jones Indices LLC via FRED",
            "series_url": SERIES_URL,
            "download_url": CSV_URL,
            "raw_sha256": raw_sha256,
            "raw_size_bytes": len(raw),
            "price_definition": "daily close of the price index, in index points",
        },
        "requested_start": START.isoformat(),
        "requested_end": END.isoformat(),
        "holdout_start": SPLIT.isoformat(),
        "trading_days_per_year": TRADING_DAYS_PER_YEAR,
        "coverage": audit,
        "splits": split_metadata,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=output.parent) as temp:
        snapshot = Path(temp) / "snapshot"
        snapshot.mkdir()
        for name, rows in splits.items():
            with (snapshot / f"{name}.csv").open("w", encoding="utf-8", newline="") as file:
                writer = csv.writer(file, lineterminator="\n")
                writer.writerow(("date", "close_index_points", "log_return"))
                writer.writerows(rows)
        (snapshot / "metadata.json").write_text(
            json.dumps(metadata, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
            encoding="utf-8",
        )
        snapshot.rename(output)
    return metadata


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--raw", type=Path, default=Path("data/raw/fred-sp500.csv"))
    parser.add_argument("--output", type=Path, default=Path("data/processed/fred-sp500"))
    parser.add_argument("--download", action="store_true", help="fetch FRED CSV if --raw is absent")
    parser.add_argument("--expected-sha256", help="reject differing source bytes")
    args = parser.parse_args()
    try:
        fetch = args.download and not args.raw.exists()
        if fetch:
            with urlopen(CSV_URL, timeout=30) as response:
                raw = response.read()
        else:
            raw = args.raw.read_bytes()
        metadata = prepare(raw, args.output, args.expected_sha256)
        if fetch:
            args.raw.parent.mkdir(parents=True, exist_ok=True)
            with args.raw.open("xb") as file:
                file.write(raw)
    except (OSError, ValueError, csv.Error) as error:
        parser.exit(1, f"FRED SP500 preparation failed: {error}\n")
    print(f"Wrote {args.output}; source SHA256: {metadata['source']['raw_sha256']}")


if __name__ == "__main__":
    main()
