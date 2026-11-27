"""Check that the src-layout package is installed, not imported from the checkout."""

# AI assistance: OpenAI ChatGPT, 2026-11-27 (initial packaging smoke test).

import subprocess
import sys
from pathlib import Path


def test_installed_package_imports_outside_checkout(tmp_path: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            "-I",
            "-c",
            "import finmath; from importlib.metadata import version; version('finmath-abm')",
        ],
        cwd=tmp_path,
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
