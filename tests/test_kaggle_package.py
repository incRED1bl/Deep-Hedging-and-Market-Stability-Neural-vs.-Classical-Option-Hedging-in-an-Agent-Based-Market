# AI assistance: OpenAI ChatGPT. All input files are synthetic fixtures.
import json
from pathlib import Path

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
