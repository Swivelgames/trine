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
	pub fn run(
		self,
		context: CliViewContext,
		cancellation: Cancellation,
	) -> Result<DispatchReport, InvocationError> {
		Ok(self.prepare(context, cancellation)?.run())
	}

	pub fn prepare(
		self,
		context: CliViewContext,
		cancellation: Cancellation,
	) -> Result<PreparedCliExecution, InvocationError> {
		(self.prepare)(context, self.json, cancellation, self.renderer)
	}
}

pub struct PreparedCliExecution {
	pub(crate) cancellation: Cancellation,
	pub(crate) run: Box<dyn FnOnce() -> DispatchReport + Send>,
}

impl PreparedCliExecution {
	pub fn cancellation(&self) -> Cancellation {
		self.cancellation.clone()
	}

	pub fn run(self) -> DispatchReport {
		(self.run)()
	}
}

pub type AsyncStructuredReport = StructuredReport;

#[must_use = "retain and await execution through settlement"]
pub struct PreparedStructured {
	pub(super) cancellation: Cancellation,
	pub(super) future:
		std::pin::Pin<Box<dyn std::future::Future<Output = AsyncStructuredReport> + Send>>,
}

impl PreparedStructured {
	pub fn cancellation(&self) -> Cancellation {
		self.cancellation.clone()
	}

	pub async fn run(self) -> AsyncStructuredReport {
		self.future.await
	}
}

pub struct StructuredReport {
	pub execution: DispatchReport,
	pub transcript: Transcript,
}
