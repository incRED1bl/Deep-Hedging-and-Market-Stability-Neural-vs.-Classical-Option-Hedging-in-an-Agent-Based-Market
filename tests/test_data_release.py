import json
import struct
from pathlib import Path

import pytest

from finmath.data_release import read_seeds, sha256_file, verify_release


def test_seed_round_trip_at_integer_boundaries(tmp_path: Path) -> None:
    seeds = (0, 2**53 - 1, 2**53, 2**53 + 1, 2**63 - 1, 2**63, 2**64 - 1)
    path = tmp_path / "train-seeds.json"
    path.write_text(json.dumps([str(seed) for seed in seeds]), encoding="utf-8")
    unsigned = tmp_path / "seeds.u64"
    layout = struct.Struct(f"<{len(seeds)}Q")

    unsigned.write_bytes(layout.pack(*read_seeds(path)))

    assert layout.unpack(unsigned.read_bytes()) == seeds


@pytest.mark.parametrize("values", [[2**63], ["-1"], ["01"], [str(2**64)], ["1", "1"]])
def test_rejects_invalid_seed_arrays(tmp_path: Path, values: list[int | str]) -> None:
    path = tmp_path / "train-seeds.json"
    path.write_text(json.dumps(values), encoding="utf-8")

    with pytest.raises(ValueError, match="seed"):
        read_seeds(path)


def test_verifies_pinned_manifest_and_exact_payload(tmp_path: Path) -> None:
    release = tmp_path / "release"
    release.mkdir()
    files = {
        "calibration.json": "{}",
        "simulation-config.json": '{"seed": 9223372036854775808}',
        "train-seeds.json": '["9223372036854775808"]',
        "validation-seeds.json": '["18446744073709551615"]',
        "DATA_CARD.md": "Fixture\n",
    }
    for name, contents in files.items():
        (release / name).write_text(contents, encoding="utf-8")
    pin = {"release_id": "fixture", "generator_commit": "a" * 40}
    manifest = {
        **pin,
        "schema_version": 1,
        "purpose": "train_validation",
        "generation_mode": "interactive_seeded",
        "calibration_file": "calibration.json",
        "simulation_config_file": "simulation-config.json",
        "final_test_included": False,
        "raw_market_data_included": False,
        "splits": {
            name: {"seeds_file": f"{name}-seeds.json", "episode_count": 1}
            for name in ("train", "validation")
        },
        "files": [
            {
                "path": name,
                "size_bytes": (release / name).stat().st_size,
                "sha256": sha256_file(release / name),
            }
            for name in files
        ],
    }
    manifest_path = release / "manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    pin["manifest_sha256"] = sha256_file(manifest_path)
    pin_path = tmp_path / "artifact-pin.json"
    pin_path.write_text(json.dumps(pin), encoding="utf-8")

    verify_release(release, pin_path)

    extra = release / "old-version.npz"
    extra.touch()
    with pytest.raises(ValueError, match="downloaded files differ"):
        verify_release(release, pin_path)
    extra.unlink()

    (release / "calibration.json").write_text("[]", encoding="utf-8")
    with pytest.raises(ValueError, match="calibration.json: size or SHA256"):
        verify_release(release, pin_path)

    manifest_path.write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError, match="manifest SHA256 differs"):
        verify_release(release, pin_path)
