"""Exercise the in-process structured presentation example, not a protocol server."""
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
env = dict(os.environ, CARGO_TARGET_DIR=str(root / "target"))
result = subprocess.run(
	["cargo", "run", "--quiet", "--example", "shared_registration"],
	cwd=root, env=env, text=True, capture_output=True, check=True,
)
assert "RPC progress: 2" in result.stdout, result.stdout
assert "RPC terminal: Some(Success" in result.stdout, result.stdout
assert "MCP custom terminal: Some(Success" in result.stdout, result.stdout
assert "1 task(s)" in result.stdout, result.stdout
print("Shared registration smoke passed: default RPC and custom MCP presentation.")
