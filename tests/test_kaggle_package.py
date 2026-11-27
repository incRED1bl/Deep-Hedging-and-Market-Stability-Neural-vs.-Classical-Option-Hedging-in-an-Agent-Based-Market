# AI assistance: OpenAI ChatGPT. All input files are synthetic fixtures.
import json
from pathlib import Path

import numpy as np
import pytest

from finmath.data_release import verify_release
from finmath.kaggle_package import stage_release


@pytest.fixture
def inputs(tmp_path: Path) -> tuple[Path, dict]:
    source = tmp_path / "source"
    source.mkdir()
    files = {
        "calibration.json": '{"purpose":"fixture"}',
        "simulation-config.json": '{"schema_version":1}',
        "train-seeds.json": '["20000","20001"]',
        "validation-seeds.json": '["30000"]',
        "DATA_CARD.md": "# Synthetic fixture\n",
    }
    for name, text in files.items():
        (source / name).write_text(text)
    protocol = tmp_path / "protocol.json"
    protocol.write_text(
        '{"seed_ranges_inclusive":{"train":[20000,29999],"validation":[30000,30049]}}'
    )
    lockfile = tmp_path / "uv.lock"
    lockfile.write_text("version = 1\n")
    return source, {
        "dataset": "owner/fixture-v1",
        "title": "Synthetic fixture",
        "mode": "interactive_seeded",
        "created_at": "2026-11-27T21:00:00Z",
        "commit": "a" * 40,
        "lockfile": lockfile,
        "protocol": protocol,
    }


def test_reproducible_package_and_external_pin(tmp_path: Path, inputs: tuple[Path, dict]) -> None:
    source, args = inputs
    output = tmp_path / "upload"
    pin = stage_release(source, output, **args)
    repeated = tmp_path / "repeated"
    assert stage_release(source, repeated, **args) == pin
    assert all(
        (repeated / file.name).read_bytes() == file.read_bytes() for file in output.iterdir()
    )
    assert not (output / "artifact-pin.json").exists()
    metadata = json.loads((output / "dataset-metadata.json").read_text())
    assert metadata["id"] == "owner/fixture-v1"
    assert metadata["licenses"] == [{"name": "copyright-authors"}]
    external_pin = tmp_path / "artifact-pin.json"
    external_pin.write_text(json.dumps(pin))
    (output / "dataset-metadata.json").unlink()  # Kaggle does not distribute upload metadata.
    verify_release(output, external_pin)


@pytest.mark.parametrize("extra", ["sp500.csv", "final-test-seeds.json", "kaggle.json"])
def test_rejects_unexpected_files(tmp_path: Path, inputs: tuple[Path, dict], extra: str) -> None:
    source, args = inputs
    (source / extra).write_text("not part of the package")
    output = tmp_path / "upload"
    with pytest.raises(ValueError, match="exactly the regular payload files"):
        stage_release(source, output, **args)
    assert not output.exists()


def test_rejects_seeds_from_a_different_split(tmp_path: Path, inputs: tuple[Path, dict]) -> None:
    source, args = inputs
    (source / "validation-seeds.json").write_text('["20000"]')
    output = tmp_path / "upload"
    with pytest.raises(ValueError, match="validation: seed outside"):
        stage_release(source, output, **args)
    assert not output.exists()


def test_rejects_placeholders_and_preserves_existing_release(
    tmp_path: Path, inputs: tuple[Path, dict]
) -> None:
    source, args = inputs
    output = tmp_path / "upload"
    stage_release(source, output, **args)
    saved = (output / "manifest.json").read_bytes()
    with pytest.raises(ValueError, match="already exists"):
        stage_release(source, output, **args)
    assert (output / "manifest.json").read_bytes() == saved

    (source / "DATA_CARD.md").write_text("REPLACE_WITH_PROVENANCE")
    with pytest.raises(ValueError, match="template placeholder"):
        stage_release(source, tmp_path / "new-release", **args)


def test_failed_repeat_preserves_the_external_pin(
    tmp_path: Path, inputs: tuple[Path, dict]
) -> None:
    source, args = inputs
    output = tmp_path / "upload"
    pin_path = tmp_path / "external.json"
    pin = stage_release(source, output, pin_output=pin_path, **args)
    assert json.loads(pin_path.read_text()) == pin
    saved = pin_path.read_bytes()
    with pytest.raises(ValueError, match="already exists"):
        stage_release(source, output, pin_output=pin_path, **args)
    assert pin_path.read_bytes() == saved


def test_pin_publication_failure_allows_retry(tmp_path: Path, inputs: tuple[Path, dict]) -> None:
    source, args = inputs
    output = tmp_path / "upload"
    pin_path = tmp_path / "external.json"
    pin_path.mkdir()
    with pytest.raises(OSError):
        stage_release(source, output, pin_output=pin_path, **args)
    assert pin_path.is_dir()
    assert not output.exists()
    pin_path.rmdir()
    stage_release(source, output, pin_output=pin_path, **args)
    assert pin_path.is_file()


def save_paths(path: Path, seeds, *, dtype=np.uint64, horizon: int = 2, price: float = 100) -> None:
    np.savez(
        path,
        episode_seed=np.asarray(seeds, dtype=dtype),
        spot=np.full((len(seeds), horizon + 1), price, dtype=np.float64),
        vol_observed=np.full((len(seeds), horizon + 1), 0.2, dtype=np.float64),
        strike=np.full(len(seeds), 100, dtype=np.float64),
    )


def test_exogenous_checks_seed_order_before_publishing(
    tmp_path: Path, inputs: tuple[Path, dict]
) -> None:
    source, args = inputs
    args["mode"] = "exogenous_pretrain"
    train, validation = (source / f"{name}-paths.npz" for name in ("train", "validation"))
    save_paths(validation, [30000])
    output = tmp_path / "upload"
    pin = tmp_path / "external.json"
    for seeds in ([20001, 20000], [20000, 2**64 - 1], [30000]):
        save_paths(train, seeds)
        with pytest.raises(ValueError, match="train-paths.npz: episode_seed differs"):
            stage_release(source, output, pin_output=pin, **args)
        assert not output.exists()
        assert not pin.exists()

    save_paths(train, [20000, 20001], dtype=np.int64)
    with pytest.raises(ValueError, match="dtype uint64"):
        stage_release(source, output, pin_output=pin, **args)
    save_paths(train, [20000, 20001], price=float("nan"))
    with pytest.raises(ValueError, match="finite float64 arrays"):
        stage_release(source, output, pin_output=pin, **args)
    save_paths(train, [20000, 20001], horizon=3)
    with pytest.raises(ValueError, match="episode horizons differ"):
        stage_release(source, output, pin_output=pin, **args)

    save_paths(train, [20000, 20001])
    stage_release(source, output, pin_output=pin, **args)
    (output / "dataset-metadata.json").unlink()
    verify_release(output, pin)
