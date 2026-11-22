//! Typed assembly before catalog erasure.

mod cli;
mod structured;

pub use cli::CliBinding;

use super::{CliBuild, CliViewContext, StructuredPrepare, ToolDescription};
use crate::{
	Action, RegistrationError,
	cli::{CliError, InputFields},
	error::Failure,
	structured::StructuredBinding,
	view::View,
};
use cli::build_cli_registration;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use structured::structured_prepare;

/// Interface methods check their own bounds before the completed entry is erased.
pub struct Entry<I, O, E, P> {
	pub(super) action: Action<I, O, E, P>,
	pub(super) namespace: Vec<String>,
	pub(super) cli: Option<Box<CliBuild>>,
	pub(super) rpc: Option<Arc<StructuredPrepare>>,
	pub(super) mcp: Option<Arc<StructuredPrepare>>,
	pub(super) tool_description: Option<ToolDescription>,
	pub(super) mcp_observation: bool,
	pub(super) duplicate: bool,
}

impl<I, O, E, P> Entry<I, O, E, P> {
	/// Start an entry for an action. Enable at least one interface before registering it.
	pub fn new(action: Action<I, O, E, P>) -> Self {
		Self {
			action,
			namespace: vec![],
			cli: None,
			rpc: None,
			mcp: None,
			tool_description: None,
			mcp_observation: false,
			duplicate: false,
		}
	}

	/// Allow this read-only tool to use the MCP host's reserved observation capacity.
	pub fn mcp_observation(mut self) -> Self {
		self.mcp_observation = true;
		self
	}

	/// Set the namespace segments preceding the action name in its registered path.
	pub fn namespace(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
		self.namespace = segments.into_iter().map(Into::into).collect();
		self
	}
}

impl<I: Send + 'static, O: Send + Sync + 'static, E: Send + Sync + 'static, P: Send + 'static>
	Entry<I, O, E, P>
{
	/// Register human and JSON presentation with one action-owned exit policy.
	///
	/// Human views render output; they cannot supply a second completion policy.
	///
	/// ```compile_fail
	/// use trine::{Entry, application::CliBinding, view::View, error::Failure};
	/// fn register<V>(entry: Entry<(), (), (), ()>, binding: CliBinding)
	/// where V: View<(), (), (), Completion = u8, Error = Failure> + 'static {
	///     entry.cli(binding, |_| -> Result<V, Failure> { unreachable!() }, |_| 0);
	/// }
	/// ```
	///
	/// Output and domain errors must support the standard JSON presentation.
	///
	/// ```compile_fail
	/// use trine::{Entry, application::CliBinding, view::View, error::Failure};
	/// struct NotSerializable;
	/// fn register<V>(entry: Entry<(), NotSerializable, (), ()>, binding: CliBinding)
	/// where V: View<NotSerializable, (), (), Completion = (), Error = Failure> + 'static {
	///     entry.cli(binding, |_| -> Result<V, Failure> { unreachable!() }, |_| 0);
	/// }
	/// ```
	pub fn cli<V, F>(
		self,
		binding: CliBinding,
		factory: F,
		exit: impl Fn(&Result<O, E>) -> u8 + Send + Sync + 'static,
	) -> Self
	where
		I: DeserializeOwned,
		O: serde::Serialize,
		E: serde::Serialize,
		V: View<O, E, P, Completion = ()> + 'static,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(CliViewContext) -> Result<V, Failure> + Send + Sync + 'static,
	{
		self.cli_with_input(binding, InputFields::deserialize, factory, exit)
	}

	/// JSON presentation also supports explicit native input assembly.
	pub fn cli_with_input<V, F, A>(
		mut self,
		binding: CliBinding,
		assemble: A,
		factory: F,
		exit: impl Fn(&Result<O, E>) -> u8 + Send + Sync + 'static,
	) -> Self
	where
		O: serde::Serialize,
		E: serde::Serialize,
		V: View<O, E, P, Completion = ()> + 'static,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(CliViewContext) -> Result<V, Failure> + Send + Sync + 'static,
		A: Fn(InputFields) -> Result<I, CliError> + Send + Sync + 'static,
	{
		self.duplicate |= self.cli.is_some();
		let action = self.action.clone();
		let projection = super::json::JsonProjection::new(exit);
		self.cli = Some(Box::new(move |program, version| {
			build_cli_registration(
				program, version, action, binding, assemble, factory, projection,
			)
		}));
		self
	}

	/// Expose this action through RPC with the supplied structured view binding.
	pub fn rpc(mut self, binding: StructuredBinding<O, E, P>) -> Self
	where
		I: DeserializeOwned,
	{
		self.duplicate |= self.rpc.is_some();
		self.rpc = Some(structured_prepare(
			self.action.clone(),
			binding,
			std::convert::identity,
		));
		self
	}

	/// Expose this action as an MCP tool using the action's input type and description.
	pub fn mcp(mut self, binding: StructuredBinding<O, E, P>) -> Self
	where
		I: DeserializeOwned,
	{
		self.duplicate |= self.mcp.is_some();
		self.mcp = Some(structured_prepare(
			self.action.clone(),
			binding,
			std::convert::identity,
		));
		self
	}

	/// Publish a narrower MCP input and translate it into the shared action's input.
	pub fn mcp_with_input<T>(
		mut self,
		binding: StructuredBinding<O, E, P>,
		description: &str,
		assemble: impl Fn(T) -> I + Send + Sync + 'static,
	) -> Result<Self, RegistrationError>
	where
		T: DeserializeOwned + schemars::JsonSchema + 'static,
	{
		if description.trim().is_empty() {
			return Err(RegistrationError("MCP tool requires a description".into()));
		}
		self.duplicate |= self.mcp.is_some();
		self.tool_description = Some(ToolDescription {
			description: description.to_owned(),
			input_schema: schemars::schema_for!(T).to_value(),
		});
		self.mcp = Some(structured_prepare(self.action.clone(), binding, assemble));
		Ok(self)
	}
}
