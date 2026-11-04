"""Exercise both registered actions and native arguments without real Git effects."""

import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"


def main():
	environment = dict(os.environ, CARGO_TARGET_DIR=str(TARGET))
	subprocess.run(
		["cargo", "build", "--example", "workyard_registry"],
		cwd=ROOT, env=environment, check=True,
	)
	binary = TARGET / "debug" / "examples" / (
		"workyard_registry.exe" if os.name == "nt" else "workyard_registry"
	)

	def run(*arguments):
		return subprocess.run(
			[binary, *arguments], cwd=ROOT, capture_output=True, timeout=10
		)

	for flag in ["--help", "--version"]:
		result = run(flag)
		assert result.returncode == 0, result.stderr
		assert result.stderr == b"", result.stderr

	result = run("create", "feat/payments", "--repo=api", "--repo=web", "--no-fetch")
	assert result.returncode == 0, result.stderr
	assert json.loads(result.stdout)["completed"] == ["api", "web"]
	assert b"create worktree" in result.stderr

	result = run("checkout", "feat/payments", "--repo=api", "--no-fetch")
	assert result.returncode == 0, result.stderr
	assert json.loads(result.stdout)["completed"] == ["api"]

	result = run("list", "--state-dir=.workyard")
	assert result.returncode == 0, result.stderr
	assert json.loads(result.stdout)["outcome"] == {
		"state_dir": ".workyard", "tasks": ["feat/payments"]
	}
	assert result.stderr == b"", result.stderr

	result = run("create", "feature", "--repo=api", "--no-fetch", "--pull")
	assert result.returncode != 0
	assert json.loads(result.stdout)["error"]["code"] == "invalid_input"
	assert result.stderr == b"", result.stderr

	if os.name == "posix":
		result = run("list", b"--state-dir=a\xff")
		assert result.returncode == 0, result.stderr
		assert json.loads(result.stdout)["outcome"]["state_dir"] == {
			"platform": "unix", "bytes": "Yf8="
		}

	print("Registry smoke tests passed: create, checkout, list, errors and native paths.")


if __name__ == "__main__":
	main()
