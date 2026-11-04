"""Build and smoke-test the executable simulation without touching repositories."""

import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"


def run_example(*arguments):
	binary = TARGET / "debug" / "examples" / (
		"workyard_create.exe" if os.name == "nt" else "workyard_create"
	)
	return subprocess.run(
		[binary, *arguments], cwd=ROOT, capture_output=True, text=True, timeout=10
	)


def main():
	environment = dict(os.environ, CARGO_TARGET_DIR=str(TARGET))
	subprocess.run(
		["cargo", "build", "--example", "workyard_create"],
		cwd=ROOT, env=environment, check=True,
	)

	success = run_example("create", "feat/payments", "--repo=api", "--repo=web", "--no-fetch")
	assert success.returncode == 0, success.stderr
	assert json.loads(success.stdout) == {
		"status": "completed",
		"outcome": {
			"task": "feat/payments", "completed": ["api", "web"], "remaining": [],
		},
	}
	assert success.stderr.splitlines()[1:] == [
		"api: create worktree", "api: complete", "web: create worktree", "web: complete",
	]

	failure = run_example("create", "feat/payments", "--repo=api", "--no-fetch", "--pull")
	assert failure.returncode != 0, failure.stdout
	result = json.loads(failure.stdout)
	assert result["status"] == "invalid", result
	assert "mutually exclusive" in result["message"], result
	assert result["remedy"], result
	assert failure.stderr.splitlines() == [
		"Simulation only: no repositories or workspace state will be changed."
	], failure.stderr
	print("Executable smoke tests passed: JSON stdout, stderr progress, and domain failure exit.")


if __name__ == "__main__":
	main()
