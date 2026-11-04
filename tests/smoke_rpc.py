"""Observe live RPC through real subprocess pipes; no Git effects."""
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading

ROOT = Path(__file__).resolve().parents[1]
ENV = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target"))
subprocess.run(["cargo", "build", "--example", "workyard_rpc"], cwd=ROOT, env=ENV, check=True)
BINARY = ROOT / "target/debug/examples" / ("workyard_rpc.exe" if os.name == "nt" else "workyard_rpc")


def invoke(identifier, action, inputs, observe=False):
	return {"jsonrpc": "2.0", "id": identifier, "method": "invoke", "params": {"action": [action], "input": inputs, "observe": observe}}


def start():
	process = subprocess.Popen([BINARY], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
	messages = queue.Queue()
	def read():
		for line in process.stdout:
			messages.put(json.loads(line))
	threading.Thread(target=read, daemon=True).start()
	return process, messages


def send(process, message):
	process.stdin.write(json.dumps(message) + "\n")
	process.stdin.flush()


process, messages = start()
try:
	request = invoke(1, "create", {"task": "feat/rpc", "repositories": [{"path": "api"}, {"path": "web"}]}, True)
	send(process, request)
	first = messages.get(timeout=5)
	assert first["method"] == "progress", first
	assert first["params"]["sequence"] == 1
	# stdin remains open: this proves streaming, not EOF-triggered transcript output.
	send(process, request)
	send(process, invoke(2, "list", {"state_dir": ".workyard"}))
	send(process, {"jsonrpc": "2.0", "method": "cancel", "params": {"id": 1}})
	responses = []
	while not (any(m.get("id") == 1 for m in responses) and any(m.get("id") == 2 for m in responses) and any(m.get("method") == "rejected" for m in responses)):
		responses.append(messages.get(timeout=5))
	terminal = next(m for m in responses if m.get("id") == 1)
	assert terminal["result"]["status"] == "cancelled", terminal
	assert sum(m.get("id") == 1 for m in responses) == 1
	process.stdin.close()
	assert process.wait(timeout=5) == 0, process.stderr.read()
finally:
	if process.poll() is None:
		process.kill()
		process.wait()

request = invoke("eof", "list", {"state_dir": ".workyard"})
result = subprocess.run([BINARY], input=json.dumps(request), text=True, capture_output=True, timeout=5)
assert result.returncode == 0, result.stderr
assert json.loads(result.stdout)["id"] == "eof"

if os.name == "posix":
	process, messages = start()
	try:
		send(process, invoke("signal", "list", {"state_dir": ".workyard"}))
		messages.get(timeout=5)
		# Idle input remains open when shutdown occurs.
		process.send_signal(signal.SIGINT)
		assert process.wait(timeout=5) == 0
	finally:
		if process.poll() is None:
			process.kill()
			process.wait()
print("RPC smoke passed: live progress, interleaving, duplicate rejection, cancellation, EOF and signal shutdown.")
