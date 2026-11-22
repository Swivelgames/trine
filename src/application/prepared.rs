//! Host-owned prepared invocations and their settlement reports.

use super::{CliViewContext, InvocationError};
use crate::{Cancellation, registry::DispatchReport, structured::Transcript};

pub(super) type PrepareCliView = dyn FnOnce(
		CliViewContext,
		bool,
		Cancellation,
		crate::diagnostics::DiagnosticRenderer,
	) -> Result<PreparedCliExecution, InvocationError>
	+ Send;

/// Parsed CLI input. Call prepare to construct its view before execution.
pub struct PreparedCli {
	pub(super) prepare: Box<PrepareCliView>,
	pub(super) json: bool,
	pub(super) renderer: crate::diagnostics::DiagnosticRenderer,
}

impl PreparedCli {
	/// Construct the selected CLI view and run the invocation to settlement.
	pub fn run(
		self,
		context: CliViewContext,
		cancellation: Cancellation,
	) -> Result<DispatchReport, InvocationError> {
		Ok(self.prepare(context, cancellation)?.run())
	}

	/// Construct the selected view now and retain the action for later execution.
	pub fn prepare(
		self,
		context: CliViewContext,
		cancellation: Cancellation,
	) -> Result<PreparedCliExecution, InvocationError> {
		(self.prepare)(context, self.json, cancellation, self.renderer)
	}
}

/// A one-shot CLI invocation with its input, view and cancellation policy resolved.
pub struct PreparedCliExecution {
	pub(crate) cancellation: Cancellation,
	pub(crate) run: Box<dyn FnOnce() -> DispatchReport + Send>,
}

impl PreparedCliExecution {
	/// Clone the token for requesting cooperative cancellation of this invocation.
	pub fn cancellation(&self) -> Cancellation {
		self.cancellation.clone()
	}

	/// Execute on the calling thread and return the domain and presentation status.
	pub fn run(self) -> DispatchReport {
		(self.run)()
	}
}

/// Settlement report produced by a prepared structured invocation.
pub type AsyncStructuredReport = StructuredReport;

/// A decoded structured action and its view, ready for an async host to schedule.
#[must_use = "retain and await execution through settlement"]
pub struct PreparedStructured {
	pub(super) cancellation: Cancellation,
	pub(super) future:
		std::pin::Pin<Box<dyn std::future::Future<Output = AsyncStructuredReport> + Send>>,
}

impl PreparedStructured {
	/// Clone the token shared with the running handler and view.
	pub fn cancellation(&self) -> Cancellation {
		self.cancellation.clone()
	}

	/// Drive the action and presentation through settlement on the caller's executor.
	pub async fn run(self) -> AsyncStructuredReport {
		self.future.await
	}
}

/// Execution status and rendered values from an in-process structured invocation.
pub struct StructuredReport {
	/// Domain and presentation status, including framework failures.
	pub execution: DispatchReport,
	/// Progress, diagnostics and terminal payload captured for this caller.
	pub transcript: Transcript,
}
