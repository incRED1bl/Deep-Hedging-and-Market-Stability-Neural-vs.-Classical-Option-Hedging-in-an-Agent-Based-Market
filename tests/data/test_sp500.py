# AI assistance: OpenAI ChatGPT. Prices below are synthetic.
import csv
import hashlib
import json
import math
from pathlib import Path

import pytest

from finmath.data.sp500 import autocorrelations, main, prepare, read_observations

HEADER = b"observation_date,SP500\n"
SYNTHETIC = (
    HEADER
    + b"""\
2017-01-06,100
2017-01-09,110
2018-01-02,100
2019-01-02,100
2020-01-02,110
2021-01-04,100
2022-12-30,100
2023-01-02,.
2023-01-03,120
2023-01-04,125
2024-01-02,140
2025-12-31,130
"""
)


def test_split_and_returns_follow_observed_closes(tmp_path: Path) -> None:
    digest = hashlib.sha256(SYNTHETIC).hexdigest()
    output = tmp_path / "first"
    prepare(SYNTHETIC, output, digest)
    meta = json.loads((output / "metadata.json").read_text())
    with (output / "calibration.csv").open(newline="") as file:
        calibration = list(csv.DictReader(file))
    with (output / "holdout.csv").open(newline="") as file:
        holdout = list(csv.DictReader(file))

    assert meta["source"]["raw_sha256"] == digest
    assert meta["coverage"]["missing_markers_by_year"] == {"2023": 1}
    assert "2023-01-05" in meta["coverage"]["weekdays_without_csv_row"]
    assert [row["date"] for row in calibration[:2]] == ["2017-01-06", "2017-01-09"]
    assert calibration[0]["log_return"] == ""
    assert float(calibration[1]["log_return"]) == pytest.approx(math.log(1.1))
    assert float(calibration[3]["log_return"]) == 0.0
    assert calibration[-1]["date"] == "2022-12-30"
    assert holdout[0]["date"] == "2023-01-03"
    assert float(holdout[0]["log_return"]) == pytest.approx(math.log(1.2))
    assert meta["splits"]["calibration"]["return_count"] == 6
    assert meta["splits"]["holdout"]["return_count"] == 4
    assert meta["splits"]["holdout"]["annualized_volatility"] == pytest.approx(
        math.sqrt(252) * meta["splits"]["holdout"]["daily_volatility"]
    )
    second = tmp_path / "second"
    prepare(SYNTHETIC, second, digest)
    assert all(path.read_bytes() == (second / path.name).read_bytes() for path in output.iterdir())
    with pytest.raises(OSError):
        prepare(SYNTHETIC, output, digest)
    assert json.loads((output / "metadata.json").read_text()) == meta


def test_rejects_incomplete_coverage_and_changed_snapshot(tmp_path: Path) -> None:
    output = tmp_path / "results"
    incomplete = HEADER + b"2025-12-31,100\n"
    with pytest.raises(ValueError, match="does not cover every year"):
        prepare(incomplete, output, hashlib.sha256(incomplete).hexdigest())
    with pytest.raises(ValueError, match="SHA256 mismatch"):
        prepare(SYNTHETIC, output, "0" * 64)
    assert not output.exists()


@pytest.mark.parametrize(
    ("rows", "error"),
    [
        (b"2017-01-06,100\n2017-01-06,.\n", "duplicate date"),
        (b"2017-01-06,nan\n", "invalid closing price"),
        (b"2017-01-06,inf\n", "invalid closing price"),
        (b"2017-01-06,0\n", "invalid closing price"),
        (b"2017-01-06,-1\n", "invalid closing price"),
        (b"2017-01-07,100\n", "invalid closing price"),
        (b"2016-12-30,100\n", "unexpected observation date"),
        (b"2017-01-06\n", "expected exactly two fields"),
    ],
)
def test_invalid_rows(rows: bytes, error: str) -> None:
    with pytest.raises(ValueError, match=error):
        read_observations(HEADER + rows)


def test_acf_uses_full_sample_mean_and_denominator() -> None:
    assert autocorrelations([1, 2, 3])[1] == 0.0
    assert autocorrelations([1, 2, 3])[2] == -0.5
    assert autocorrelations([1, 2, 3])[3] is None
    assert autocorrelations([2, 2, 2])[1] is None


class FixtureSource:
    provider = "Independent fixture source"
    series_url = "https://example.invalid/series"
    download_url = "https://example.invalid/bytes"
    snapshot_sha256 = hashlib.sha256(SYNTHETIC).hexdigest()

    def download(self) -> bytes:
        return SYNTHETIC


def test_cli_rejects_one_missing_session_against_the_source_pin(tmp_path: Path, capsys) -> None:
    raw = tmp_path / "changed.csv"
    raw.write_bytes(SYNTHETIC.replace(b"2023-01-04,125", b"2023-01-04,."))
    output = tmp_path / "splits"
    with pytest.raises(SystemExit, match="1"):
        main(["--raw", str(raw), "--output", str(output)], source=FixtureSource())
    assert "SHA256 mismatch" in capsys.readouterr().err
    assert not output.exists()


def test_cli_retries_after_raw_write_failure_without_partial_outputs(tmp_path: Path) -> None:
    raw = tmp_path / "raw.csv"
    raw.symlink_to(tmp_path / "missing.csv")
    output = tmp_path / "splits"
    args = ["--download", "--raw", str(raw), "--output", str(output)]
    with pytest.raises(SystemExit, match="1"):
        main(args, source=FixtureSource())
    assert raw.is_symlink()
    assert not output.exists()

    raw.unlink()
    main(args, source=FixtureSource())
    assert raw.read_bytes() == SYNTHETIC
    meta = json.loads((output / "metadata.json").read_text())
    assert meta["source"]["provider"] == FixtureSource.provider
    assert meta["source"]["download_url"] == FixtureSource.download_url
