//! Invocation-scoped wire presentation and terminal delivery.

use super::{
	framing,
	outbox::{Message, Outbox},
	protocol::{self, Id},
};
use crate::{
	Cancellation,
	error::{Failure, FailureKind},
	registry::DispatchReport,
	structured::{LiveSink, Payload, SendFuture},
};
use serde_json::{Value, json};

struct Delivery {
	sequence: u64,
	terminal: bool,
}

pub(super) struct Sink {
	id: Id,
	outbox: Outbox,
	limit: usize,
	cancellation: Cancellation,
	delivery: smol::lock::Mutex<Delivery>,
}

impl Sink {
	pub(super) fn new(id: Id, outbox: Outbox, limit: usize, cancellation: Cancellation) -> Self {
		Self {
			id,
			outbox,
			limit,
			cancellation,
			delivery: smol::lock::Mutex::new(Delivery {
				sequence: 0,
				terminal: false,
			}),
		}
	}

	async fn send_terminal(&self, value: Value) -> Result<(), Failure> {
		let mut delivery = self.delivery.lock().await;
		if delivery.terminal {
			return Err(Failure::message(
				FailureKind::ContractViolation,
				"terminal already committed",
			));
		}

		let bytes = framing::encode(&value, self.limit)?;
		self.outbox
			.send(
				Message {
					bytes,
					terminal: Some(self.id.clone()),
				},
				None,
			)
			.await?;

		delivery.terminal = true;
		Ok(())
	}

	pub(super) async fn framework(
		&self,
		report: &mut DispatchReport,
		renderer: &crate::diagnostics::DiagnosticRenderer,
	) {
		if self.delivery.lock().await.terminal {
			return;
		}
		let Some(diagnostic) = crate::diagnostics::Diagnostic::from_report(report) else {
			return;
		};

		let execution = if report.execution_failure.is_some() {
			"abnormal"
		} else if report.domain_succeeded {
			"returned_ok"
		} else {
			"returned_error"
		};
		let kind = match report.presentation_failure() {
			Some(FailureKind::OutputTooLarge) => "output_too_large",
			Some(FailureKind::EncodingFailed) => "encoding_failed",
			Some(_) => "presentation_failed",
			None => "execution_failed",
		};
		let envelope = |message: &str| {
			let mut value = protocol::error(self.id.value(), -32603, kind, message);
			value["error"]["data"]["execution"] = json!(execution);
			value
		};

		report.diagnostics.attempted = true;
		let result = match renderer.render(&diagnostic) {
			Ok(message) => self.send_terminal(envelope(&message)).await,
			Err(error) => Err(error),
		};

		if let Err(error) = result {
			report.diagnostics.failure = Some(error);
			report.diagnostics.fallback_attempted = true;
			report.diagnostics.fallback_failure = self
				.send_terminal(envelope(
					"Invocation reporting failed; effects may be incomplete",
				))
				.await
				.err();
		}
	}
}

impl LiveSink for Sink {
	fn progress(&self, event: Value) -> SendFuture<'_> {
		Box::pin(async move {
			let mut delivery = self.delivery.lock().await;
			if delivery.terminal {
				return Err(Failure::message(
					FailureKind::ContractViolation,
					"progress after terminal",
				));
			}

			let sequence = delivery.sequence.checked_add(1).ok_or_else(|| {
				Failure::message(FailureKind::EncodingFailed, "sequence exhausted")
			})?;
			let value = json!({
				"jsonrpc": "2.0",
				"method": "progress",
				"params": {
					"id": self.id.value(),
					"sequence": sequence,
					"event": event,
				},
			});

			let bytes = framing::encode(&value, self.limit)?;
			let admitted = self
				.outbox
				.send(
					Message {
						bytes,
						terminal: None,
					},
					Some(&self.cancellation),
				)
				.await?;

			if admitted {
				delivery.sequence = sequence;
			}
			Ok(())
		})
	}

	fn terminal(&self, payload: Payload) -> SendFuture<'_> {
		Box::pin(async move {
			let value = match payload {
				Payload::Success(value) => {
					json!({
						"jsonrpc": "2.0",
						"id": self.id.value(),
						"result": value,
					})
				}
				Payload::DomainFailure(data) => {
					json!({
						"jsonrpc": "2.0",
						"id": self.id.value(),
						"error": {
							"code": 1,
							"message": "Action failed",
							"data": data,
						},
					})
				}
				Payload::RpcError {
					code,
					message,
					data,
				} => {
					if (-32768..=-32000).contains(&code) {
						return Err(Failure::message(
							FailureKind::ContractViolation,
							"reserved RPC error code",
						));
					}
					json!({
						"jsonrpc": "2.0",
						"id": self.id.value(),
						"error": {
							"code": code,
							"message": message,
							"data": data,
						},
					})
				}
				Payload::FrameworkFailure { kind, message } => {
					protocol::error(self.id.value(), -32603, &format!("{kind:?}"), &message)
				}
			};

			self.send_terminal(value).await
		})
	}
}
