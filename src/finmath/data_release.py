"""Verify a downloaded train/validation release against its external pin."""

import argparse
import hashlib
import json
import re
from pathlib import Path

BASE_FILES = frozenset(
    {
        "calibration.json",
        "simulation-config.json",
        "train-seeds.json",
        "validation-seeds.json",
        "DATA_CARD.md",
    }
)
PAYLOADS = {
    "interactive_seeded": BASE_FILES,
    "exogenous_pretrain": BASE_FILES | {"train-paths.npz", "validation-paths.npz"},
}
MAX_SEED = 2**64 - 1


def read_seeds(path: Path) -> list[int]:
    """Read canonical decimal strings as exact integers in the Rust u64 range."""
    return parse_seeds(json.loads(path.read_text(encoding="utf-8")), source=path.name)


def parse_seeds(values: object, *, source: str) -> list[int]:
    """Validate a JSON seed array before converting its canonical decimal strings."""
    if not isinstance(values, list) or not values:
        raise ValueError(f"{source}: expected a nonempty array of seeds")

    seeds = []
    for value in values:
        if not isinstance(value, str) or re.fullmatch(r"0|[1-9][0-9]{0,19}", value) is None:
            raise ValueError(f"{source}: expected unsigned decimal seed strings")
        seed = int(value)
        if seed > MAX_SEED:
            raise ValueError(f"{source}: seed exceeds the u64 range")
        seeds.append(seed)
    if len(seeds) != len(set(seeds)):
        raise ValueError(f"{source}: duplicate seeds")
    return seeds


def sha256_file(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def verify_release(directory: Path, pin_path: Path) -> None:
    pin = json.loads(pin_path.read_text(encoding="utf-8"))
    entries = {path.name: path for path in directory.iterdir()}
    if any(path.is_symlink() or not path.is_file() for path in entries.values()):
        raise ValueError("release must contain only regular files at its root")

    manifest_path = entries["manifest.json"]
    manifest_bytes = manifest_path.read_bytes()
    if hashlib.sha256(manifest_bytes).hexdigest() != pin["manifest_sha256"]:
        raise ValueError("manifest SHA256 differs from the external pin")

    manifest = json.loads(manifest_bytes)
    if (
        manifest["schema_version"] != 1
        or manifest["purpose"] != "train_validation"
        or manifest["final_test_included"] is not False
        or manifest["raw_market_data_included"] is not False
    ):
        raise ValueError("expected a schema v1 train/validation release")
    for field in ("release_id", "generator_commit"):
        if manifest[field] != pin[field]:
            raise ValueError(f"{field} differs from the external pin")

    expected = PAYLOADS[manifest["generation_mode"]]
    listed = {entry["path"]: entry for entry in manifest["files"]}
    if listed.keys() != expected or len(listed) != len(manifest["files"]):
        raise ValueError("manifest files must match the generation mode without duplicates")
    if entries.keys() != expected | {"manifest.json"}:
        raise ValueError("downloaded files differ from the manifest")
    for filename, entry in listed.items():
        path = entries[filename]
        if path.stat().st_size != entry["size_bytes"] or sha256_file(path) != entry["sha256"]:
            raise ValueError(f"{filename}: size or SHA256 differs from the manifest")

    if (
        manifest["calibration_file"] != "calibration.json"
        or manifest["simulation_config_file"] != "simulation-config.json"
        or manifest["splits"].keys() != {"train", "validation"}
    ):
        raise ValueError("manifest references must match the schema v1 file names and splits")

    seeds = {}
    for name, split in manifest["splits"].items():
        filename = f"{name}-seeds.json"
        if split["seeds_file"] != filename:
            raise ValueError(f"{name}: expected seed file {filename}")
        seeds[name] = set(read_seeds(entries[filename]))
        if len(seeds[name]) != split["episode_count"]:
            raise ValueError(f"{name}: seed count differs from the manifest")
    if seeds["train"] & seeds["validation"]:
        raise ValueError("train and validation seeds overlap")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("pin", type=Path)
    args = parser.parse_args()
    try:
        verify_release(args.directory, args.pin)
    except (KeyError, AttributeError, TypeError, ValueError, OSError) as error:
        parser.exit(1, f"Release verification failed: {error}\n")
    print(f"Verified release in {args.directory}")


if __name__ == "__main__":
    main()
