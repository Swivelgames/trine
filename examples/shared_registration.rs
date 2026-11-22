//! Invoke two actions through their registered RPC and MCP views in process.

#[path = "support/shared_workyard/mod.rs"]
mod workyard;

use trine::{Cancellation, structured::Payload};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let app = workyard::application()?;
	let prepared = app.prepare_rpc(
		&["create"],
		serde_json::json!({
			"task": "feat/payments",
			"repositories": [{"path": "api"}, {"path": "web"}],
			"no_fetch": true
		}),
		true,
		Cancellation::new(),
	)?;
	let create = smol::block_on(prepared.run());
	assert!(create.execution.domain_succeeded && create.execution.presentation_succeeded);
	println!("RPC progress: {}", create.transcript.progress.len());
	println!("RPC terminal: {:?}", create.transcript.terminal);

	let list = app.invoke_mcp(
		&["list"],
		serde_json::json!({"state_dir":".workyard"}),
		false,
		Cancellation::new(),
	)?;
	assert!(list.execution.domain_succeeded && list.execution.presentation_succeeded);
	assert!(matches!(
		list.transcript.terminal,
		Some(Payload::Success(_))
	));
	println!("MCP custom terminal: {:?}", list.transcript.terminal);
	Ok(())
}
