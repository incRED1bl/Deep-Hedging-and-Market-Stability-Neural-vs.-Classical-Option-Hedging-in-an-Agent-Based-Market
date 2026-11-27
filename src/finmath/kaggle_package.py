"""Stage an allowlisted train/validation package for private Kaggle upload."""

# AI assistance: OpenAI ChatGPT. See docs/data-and-kaggle.md for the package contract.
import argparse
import json
import re
import shutil
from datetime import UTC, datetime, timedelta
from pathlib import Path
from tempfile import TemporaryDirectory

from finmath.data_release import PAYLOADS, read_seeds, sha256_file, verify_release

DATASET_ID = re.compile(r"[a-z0-9][a-z0-9_-]*/[a-z0-9][a-z0-9-]*")


def _write_json(path: Path, value: dict) -> None:
    path.write_text(
        json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8"
    )


def stage_release(
    source: Path,
    output: Path,
    *,
    dataset: str,
    title: str,
    mode: str,
    created_at: str,
    commit: str,
    lockfile: Path,
    protocol: Path,
    pin_output: Path | None = None,
) -> dict:
    """Assemble and verify payload bytes. Return metadata for the external pin."""
    if DATASET_ID.fullmatch(dataset) is None:
        raise ValueError("expected a Kaggle owner/dataset-slug")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("expected a full Git commit SHA")
    timestamp = datetime.fromisoformat(created_at)
    if timestamp.utcoffset() != timedelta(0):
        raise ValueError("created-at must be a UTC timestamp")
    if not title.strip() or "REPLACE_WITH" in title:
        raise ValueError("provide the release title")
    if mode not in PAYLOADS:
        raise ValueError(f"unknown generation mode: {mode}")
    release_id = dataset.split("/")[1]
    if output.exists() or output.is_symlink():
        raise ValueError(f"release directory already exists: {output}")
    if pin_output is not None and pin_output.resolve().is_relative_to(output.resolve()):
        raise ValueError("the external pin must be outside the release directory")
    expected = PAYLOADS[mode]
    entries = {path.name: path for path in source.iterdir()}
    if entries.keys() != expected or any(
        path.is_symlink() or not path.is_file() for path in entries.values()
    ):
        raise ValueError("source must contain exactly the regular payload files for the mode")
    seed_ranges = json.loads(protocol.read_text(encoding="utf-8"))["seed_ranges_inclusive"]
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=output.parent) as temp:
        temporary = Path(temp)
        staging = temporary / "release"
        staging.mkdir()
        for name, path in sorted(entries.items()):
            shutil.copyfile(path, staging / name)
        splits = {}
        for name in ("train", "validation"):
            seed_file = f"{name}-seeds.json"
            seeds = read_seeds(staging / seed_file)
            lower, upper = seed_ranges[name]
            if (
                type(lower) is not int
                or type(upper) is not int
                or not lower <= min(seeds) <= max(seeds) <= upper
            ):
                raise ValueError(f"{name}: seed outside the protocol split")
            splits[name] = {"seeds_file": seed_file, "episode_count": len(seeds)}
        for name in ("calibration.json", "simulation-config.json", "DATA_CARD.md"):
            text = (staging / name).read_text(encoding="utf-8")
            if not text.strip() or "REPLACE_WITH" in text:
                raise ValueError(f"{name}: empty content or template placeholder")
            if name.endswith(".json") and (
                not isinstance(data := json.loads(text), dict) or not data
            ):
                raise ValueError(f"{name}: expected a populated JSON object")
        manifest_path = staging / "manifest.json"
        _write_json(
            manifest_path,
            {
                "schema_version": 1,
                "release_id": release_id,
                "purpose": "train_validation",
                "generation_mode": mode,
                "created_at_utc": timestamp.astimezone(UTC).isoformat(),
                "generator_commit": commit,
                "uv_lock_sha256": sha256_file(lockfile),
                "calibration_file": "calibration.json",
                "simulation_config_file": "simulation-config.json",
                "splits": splits,
                "raw_market_data_included": False,
                "final_test_included": False,
                "files": [
                    {
                        "path": name,
                        "size_bytes": (staging / name).stat().st_size,
                        "sha256": sha256_file(staging / name),
                    }
                    for name in sorted(expected)
                ],
            },
        )
        pin = {
            "dataset": dataset,
            "release_id": release_id,
            "manifest_sha256": sha256_file(manifest_path),
            "generator_commit": commit,
        }
        pin_path = temporary / "artifact-pin.json"
        _write_json(pin_path, pin)
        verify_release(staging, pin_path)
        _write_json(
            staging / "dataset-metadata.json",
            {
                "id": dataset,
                "title": title.strip(),
                "description": f"Synthetic train/validation release {release_id}.",
                "licenses": [{"name": "copyright-authors"}],
            },
        )
        if pin_output is None:
            staging.rename(output)
        else:
            pin_output.parent.mkdir(parents=True, exist_ok=True)
            with TemporaryDirectory(dir=pin_output.parent) as pin_temp:
                staged_pin = Path(pin_temp) / "artifact-pin.json"
                shutil.copyfile(pin_path, staged_pin)
                staging.rename(output)
                try:
                    staged_pin.replace(pin_output)
                except OSError:
                    output.rename(staging)
                    raise
    return pin


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="directory with only synthetic payload files")
    parser.add_argument("output", type=Path, help="new upload staging directory")
    parser.add_argument("--dataset", required=True, help="Kaggle owner/slug")
    parser.add_argument("--title", required=True)
    parser.add_argument("--mode", choices=sorted(PAYLOADS), required=True)
    parser.add_argument("--created-at", required=True, help="fixed ISO-8601 UTC timestamp")
    parser.add_argument("--commit", required=True, help="full commit SHA of the generator")
    parser.add_argument("--lockfile", type=Path, default=Path("uv.lock"))
    parser.add_argument("--pin-output", type=Path, help="publish the external pin at this path")
    parser.add_argument(
        "--protocol", type=Path, required=True, help="protocol JSON with seed ranges"
    )
    try:
        pin = stage_release(**vars(parser.parse_args()))
    except (KeyError, AttributeError, TypeError, ValueError, OSError) as error:
        parser.exit(1, f"Kaggle packaging failed: {error}\n")
    print(json.dumps(pin, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
