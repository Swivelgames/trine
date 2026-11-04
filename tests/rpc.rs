use futures_lite::io::{AsyncWrite, Cursor};
use serde_json::{Value, json};
use std::{
	io,
	pin::Pin,
	sync::{Arc, Mutex},
	task::{Context, Poll},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext,
	rpc::{self, Config},
	structured::StructuredBinding,
};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl AsyncWrite for Output {
	fn poll_write(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
		bytes: &[u8],
	) -> Poll<io::Result<usize>> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Poll::Ready(Ok(bytes.len()))
	}
	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
	fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}
fn app() -> Application {
	let action = Action::new(
		"echo",
		"Echo",
		|input: Value, context: InvocationContext<Value>| async move {
			let _ = context.emit(json!({"phase":"started"})).await;
			if input == "panic" {
				panic!("handler failed");
			}
			if input == "error" {
				Err(json!({"code":"domain_error"}))
			} else {
				Ok(input)
			}
		},
	)
	.unwrap();
	let mut app = Application::new("test", "1").unwrap();
	app.register(
		Entry::new(action).rpc(StructuredBinding::serde_with_progress(
			Ok,
			std::num::NonZeroUsize::new(1).unwrap(),
		)),
	)
	.unwrap();
	app
}
fn invoke(id: Value, input: Value, observe: bool) -> Value {
	json!({"jsonrpc":"2.0","id":id,"method":"invoke","params":{"action":["echo"],"input":input,"observe":observe}})
}
fn run(input: &[u8], config: Config) -> (Vec<Value>, rpc::HostReport, Vec<(bool, bool)>) {
	let output = Output::default();
	let copy = output.clone();
	let mut outcomes = vec![];
	let report = smol::block_on(rpc::serve(
		&app(),
		Cursor::new(input.to_vec()),
		output,
		config,
		Cancellation::new(),
		|r| outcomes.push((r.domain_succeeded, r.presentation_succeeded)),
	))
	.unwrap();
	let bytes = copy.0.lock().unwrap();
	let frames = bytes
		.split(|b| *b == b'\n')
		.filter(|line| !line.is_empty())
		.map(|line| serde_json::from_slice(line).unwrap())
		.collect();
	(frames, report, outcomes)
}
fn lines(values: &[Value]) -> Vec<u8> {
	values
		.iter()
		.map(|v| format!("{v}\n"))
		.collect::<String>()
		.into_bytes()
}

#[test]
fn observed_result_is_live_ordered_and_ids_preserve_type() {
	let (frames, report, results) = run(
		&lines(&[
			invoke(json!(1), json!("first"), true),
			invoke(json!("1"), json!("second"), false),
		]),
		Config::default(),
	);
	assert_eq!(report.invocations, 2);
	assert_eq!(results, vec![(true, true), (true, true)]);
	let progress = frames
		.iter()
		.position(|v| v["method"] == "progress")
		.unwrap();
	let terminal = frames
		.iter()
		.position(|v| v["id"] == 1 && v.get("result").is_some())
		.unwrap();
	assert!(progress < terminal);
	assert_eq!(frames[progress]["params"]["sequence"], 1);
	assert!(
		frames
			.iter()
			.any(|v| v["id"] == "1" && v["result"] == "second")
	);
}
#[test]
fn duplicates_get_notification_and_never_execute_twice() {
	let request = invoke(json!(5), json!(true), false);
	let (frames, report, _) = run(&lines(&[request.clone(), request]), Config::default());
	assert_eq!(report.invocations, 1);
	assert_eq!(
		frames.iter().filter(|v| v.get("result").is_some()).count(),
		1
	);
	let rejected = frames.iter().find(|v| v["method"] == "rejected").unwrap();
	assert!(rejected.get("id").is_none());
	assert_eq!(rejected["params"]["data"]["id"], 5);
}
#[test]
fn malformed_batch_unavailable_and_notifications_do_not_execute() {
	let mut bytes = b"broken\n".to_vec();
	let mut unavailable = invoke(json!(3), Value::Null, false);
	unavailable["params"]["action"] = json!(["missing"]);
	let mut notification = invoke(json!(4), Value::Null, false);
	notification.as_object_mut().unwrap().remove("id");
	bytes.extend(lines(&[
		json!([invoke(json!(2), Value::Null, false)]),
		unavailable,
		notification,
		invoke(json!(6), json!(true), false),
	]));
	let (frames, report, _) = run(&bytes, Config::default());
	assert_eq!(report.invocations, 1);
	for code in [-32700, -32600, -32602] {
		assert!(frames.iter().any(|v| v["error"]["code"] == code));
	}
}
#[test]
fn oversized_input_recovers_at_delimiter_and_final_line_needs_no_newline() {
	let config = Config {
		input_frame_bytes: 256,
		..Config::default()
	};
	let mut input = vec![b'x'; 1000];
	input.extend(b"\r\n");
	input.extend(invoke(json!(1), json!(true), false).to_string().as_bytes());
	let (frames, report, _) = run(&input, config);
	assert_eq!(report.invocations, 1);
	assert!(
		frames
			.iter()
			.any(|v| v["error"]["data"]["code"] == "frame_too_large")
	);
	assert!(frames.iter().any(|v| v["result"] == true));
}
#[test]
fn oversize_output_preserves_success_and_uses_small_framework_error() {
	let config = Config {
		output_frame_bytes: 2048,
		id_bytes: 32,
		..Config::default()
	};
	let (frames, _, outcomes) = run(
		&lines(&[invoke(json!(1), json!("x".repeat(5000)), false)]),
		config,
	);
	assert_eq!(outcomes, vec![(true, false)]);
	assert_eq!(frames[0]["error"]["data"]["code"], "output_too_large");
	assert_eq!(frames[0]["error"]["data"]["execution"], "returned_ok");
}
#[test]
fn domain_and_abnormal_errors_keep_distinct_meanings() {
	let (frames, _, outcomes) = run(
		&lines(&[
			invoke(json!(1), json!("error"), false),
			invoke(json!(2), json!("panic"), false),
		]),
		Config::default(),
	);
	assert!(outcomes.contains(&(false, true)));
	assert!(
		frames
			.iter()
			.any(|v| v["error"]["code"] == 1 && v["error"]["data"]["code"] == "domain_error")
	);
	assert!(
		frames
			.iter()
			.any(|v| v["error"]["data"]["execution"] == "abnormal")
	);
}
#[test]
fn invalid_config_rejects_before_reading() {
	let config = Config {
		output_bytes: 1,
		..Config::default()
	};
	assert!(
		smol::block_on(rpc::serve(
			&app(),
			Cursor::new(vec![]),
			Output::default(),
			config,
			Cancellation::new(),
			|_| {}
		))
		.is_err()
	);
}

#[test]
fn custom_rpc_view_owns_application_error_and_terminal_guard() {
	use trine::{
		structured::{Payload, StructuredContext},
		view::View,
	};
	struct Custom(StructuredContext);
	impl View<bool, (), ()> for Custom {
		type Completion = ();
		type Error = trine::error::Failure;
		async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<bool, ()>) -> Result<(), Self::Error> {
			self.0
				.terminal(Payload::RpcError {
					code: 42,
					message: "Custom error".into(),
					data: json!({"reason":"custom"}),
				})
				.await?;
			self.0.terminal(Payload::Success(json!(false))).await
		}
	}
	let mut app = Application::new("test", "1").unwrap();
	app.register(
		Entry::new(
			Action::new("echo", "Echo", |_: Value, _: InvocationContext<()>| async {
				Ok::<_, ()>(true)
			})
			.unwrap(),
		)
		.rpc(StructuredBinding::custom(|context| Ok(Custom(context)))),
	)
	.unwrap();
	let output = Output::default();
	let capture = output.clone();
	let mut observed = false;
	smol::block_on(rpc::serve(
		&app,
		Cursor::new(lines(&[invoke(json!(1), Value::Null, false)])),
		output,
		Config::default(),
		Cancellation::new(),
		|report| {
			observed = true;
			assert!(report.domain_succeeded);
			assert!(!report.presentation_succeeded);
		},
	))
	.unwrap();
	assert!(observed);
	let bytes = capture.0.lock().unwrap();
	let frame: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(frame["error"]["code"], 42);
	assert_eq!(frame["error"]["data"]["reason"], "custom");
}
#[test]
fn custom_framework_renderer_is_used_for_abnormal_execution() {
	use trine::{application::Interface, diagnostics::DiagnosticRenderer};
	let mut app = app();
	app.diagnostic_renderer(
		Interface::Rpc,
		DiagnosticRenderer::new(|_| Ok("Custom framework message".into())),
	);
	let output = Output::default();
	let capture = output.clone();
	smol::block_on(rpc::serve(
		&app,
		Cursor::new(lines(&[invoke(json!(1), json!("panic"), false)])),
		output,
		Config::default(),
		Cancellation::new(),
		|report| assert!(report.diagnostics.attempted),
	))
	.unwrap();
	let bytes = capture.0.lock().unwrap();
	let frame: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(frame["error"]["message"], "Custom framework message");
}

#[test]
fn setup_failure_does_not_start_action_and_encoding_panic_preserves_result() {
	use serde::{Serialize, Serializer};
	use std::sync::atomic::{AtomicUsize, Ordering};
	use trine::structured::StructuredContext;
	struct Panicking;
	impl Serialize for Panicking {
		fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
			panic!("serializer panic");
		}
	}
	let calls = Arc::new(AtomicUsize::new(0));
	let observed = calls.clone();
	let mut app = Application::new("test", "1").unwrap();
	let action = Action::new("echo", "Echo", move |_: Value, _: InvocationContext<()>| {
		observed.fetch_add(1, Ordering::SeqCst);
		async { Ok::<_, ()>(Panicking) }
	})
	.unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::serde()))
		.unwrap();
	let output = Output::default();
	let capture = output.clone();
	smol::block_on(rpc::serve(
		&app,
		Cursor::new(lines(&[invoke(json!(1), Value::Null, false)])),
		output,
		Config::default(),
		Cancellation::new(),
		|report| {
			assert!(report.domain_succeeded);
			assert!(!report.presentation_succeeded);
		},
	))
	.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let bytes = capture.0.lock().unwrap();
	let value: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(value["error"]["data"]["execution"], "returned_ok");
	struct Empty;
	impl trine::view::View<(), (), ()> for Empty {
		type Completion = ();
		type Error = trine::error::Failure;
		async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Self::Error> {
			Ok(())
		}
	}
	let observed = calls.clone();
	let mut app = Application::new("test", "1").unwrap();
	app.register(
		Entry::new(
			Action::new("echo", "Echo", move |_: Value, _: InvocationContext<()>| {
				observed.fetch_add(1, Ordering::SeqCst);
				async { Ok::<_, ()>(()) }
			})
			.unwrap(),
		)
		.rpc(StructuredBinding::custom(
			|_: StructuredContext| -> Result<Empty, trine::error::Failure> {
				panic!("factory panic")
			},
		)),
	)
	.unwrap();
	let output = Output::default();
	let capture = output.clone();
	smol::block_on(rpc::serve(
		&app,
		Cursor::new(lines(&[invoke(json!(2), Value::Null, false)])),
		output,
		Config::default(),
		Cancellation::new(),
		|_| panic!("rejected action settled"),
	))
	.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let bytes = capture.0.lock().unwrap();
	let value: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(value["error"]["data"]["code"], "view_setup_failed");
}

#[test]
fn custom_rpc_error_reserved_endpoints_fail_but_adjacent_codes_are_allowed() {
	use trine::{
		structured::{Payload, StructuredContext},
		view::View,
	};
	struct ErrorView(StructuredContext, i64);
	impl View<bool, (), ()> for ErrorView {
		type Completion = ();
		type Error = trine::error::Failure;
		async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<bool, ()>) -> Result<(), Self::Error> {
			self.0
				.terminal(Payload::RpcError {
					code: self.1,
					message: "custom".into(),
					data: Value::Null,
				})
				.await
		}
	}
	for (code, reserved) in [
		(-32769, false),
		(-32768, true),
		(-32000, true),
		(-31999, false),
	] {
		let mut app = Application::new("test", "1").unwrap();
		let action = Action::new("echo", "Echo", |_: Value, _: InvocationContext<()>| async {
			Ok::<_, ()>(true)
		})
		.unwrap();
		app.register(
			Entry::new(action).rpc(StructuredBinding::custom(move |context| {
				Ok(ErrorView(context, code))
			})),
		)
		.unwrap();
		let output = Output::default();
		let capture = output.clone();
		smol::block_on(rpc::serve(
			&app,
			Cursor::new(lines(&[invoke(json!(1), Value::Null, false)])),
			output,
			Config::default(),
			Cancellation::new(),
			|report| {
				assert!(report.domain_succeeded);
				assert_eq!(report.presentation_succeeded, !reserved);
			},
		))
		.unwrap();
		let bytes = capture.0.lock().unwrap();
		// Parsing the complete stream as one value also rejects a second response.
		let frame: Value = serde_json::from_slice(&bytes).unwrap();
		assert_eq!(frame["error"]["code"], if reserved { -32603 } else { code });
		if reserved {
			assert_eq!(frame["error"]["data"]["execution"], "returned_ok");
		}
	}
}
