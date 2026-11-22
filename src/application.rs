//! Shared action catalog with explicit interface projections.

mod entry;
mod invocation_error;
pub mod json;
mod prepared;
mod presentation;
mod routing;

pub use entry::{CliBinding, Entry};
pub use invocation_error::InvocationError;
pub use prepared::{
	AsyncStructuredReport, PreparedCli, PreparedCliExecution, PreparedStructured, StructuredReport,
};
pub use presentation::{CliCompletion, CliViewContext, Output};

use crate::{
	Cancellation, RegistrationError,
	action::validate_name,
	cli::{CliError, Parsed},
	structured::{Destination, LiveSink, ProgressSink},
};
use clap::Command;
use routing::CliRoutes;
use serde_json::Value;
use std::{collections::BTreeMap, ffi::OsString, sync::Arc};

/// The presentation interface used to discover actions or configure diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
	/// Human-facing commands, including direct CLI JSON output.
	Cli,
	/// Named action invocations over the RPC adapter.
	Rpc,
	/// Agent tools exposed through the Model Context Protocol.
	Mcp,
}

type CliPrepare = dyn Fn(Vec<OsString>) -> Result<Parsed<PreparedCli>, CliError> + Send + Sync;

struct CliRegistration {
	commands: Vec<Command>,
	prepare: Arc<CliPrepare>,
}

type CliBuild = dyn FnOnce(&str, &str) -> Result<CliRegistration, RegistrationError>;
type StructuredFactory = dyn Fn(
		Destination,
		Value,
		bool,
		Cancellation,
		crate::diagnostics::DiagnosticRenderer,
	) -> Result<PreparedStructured, InvocationError>
	+ Send
	+ Sync;

struct StructuredPrepare {
	prepare: Box<StructuredFactory>,
}

/// Discovery metadata for one action on a particular interface.
#[derive(Clone, Debug)]
pub struct ActionInfo {
	/// Namespace segments followed by the action name, in invocation order.
	pub path: Vec<String>,
	/// Human-readable summary advertised to callers.
	pub description: String,
	/// JSON Schema describing the input accepted on this interface.
	pub input_schema: Value,
}

struct ToolDescription {
	description: String,
	input_schema: Value,
}

/// A catalog lookup supplies admission policy before constructing the tool's view.
pub(crate) struct McpTool<'a> {
	pub(crate) observation: bool,
	binding: &'a StructuredPrepare,
	renderer: &'a crate::diagnostics::DiagnosticRenderer,
}

impl McpTool<'_> {
	pub(crate) fn prepare(
		self,
		input: Value,
		cancellation: Cancellation,
		progress: Option<Arc<impl ProgressSink + 'static>>,
	) -> Result<PreparedStructured, InvocationError> {
		let observe = progress.is_some();
		let destination =
			progress.map_or(Destination::Transcript, |sink| Destination::Progress(sink));
		(self.binding.prepare)(
			destination,
			input,
			observe,
			cancellation,
			self.renderer.clone(),
		)
	}
}

struct Registered {
	info: ActionInfo,
	tool_info: Option<ActionInfo>,
	cli: bool,
	rpc: Option<Arc<StructuredPrepare>>,
	mcp: Option<Arc<StructuredPrepare>>,
	mcp_observation: bool,
}

#[derive(Default)]
struct Renderers {
	cli: crate::diagnostics::DiagnosticRenderer,
	rpc: crate::diagnostics::DiagnosticRenderer,
	mcp: crate::diagnostics::DiagnosticRenderer,
}

impl Renderers {
	fn get(&self, interface: Interface) -> &crate::diagnostics::DiagnosticRenderer {
		match interface {
			Interface::Cli => &self.cli,
			Interface::Rpc => &self.rpc,
			Interface::Mcp => &self.mcp,
		}
	}

	fn get_mut(&mut self, interface: Interface) -> &mut crate::diagnostics::DiagnosticRenderer {
		match interface {
			Interface::Cli => &mut self.cli,
			Interface::Rpc => &mut self.rpc,
			Interface::Mcp => &mut self.mcp,
		}
	}
}

/// A catalog that routes CLI commands, RPC requests and MCP tool calls to typed actions.
pub struct Application {
	diagnostics: Renderers,
	program: String,
	version: String,
	entries: BTreeMap<Vec<String>, Registered>,
	cli_routes: CliRoutes,
}

impl Application {
	/// Create an empty catalog. The program name follows the same naming rules as an action;
	/// the version appears in help and host identity.
	pub fn new(
		program: impl Into<String>,
		version: impl Into<String>,
	) -> Result<Self, RegistrationError> {
		let program = program.into();
		validate_name(&program)?;

		Ok(Self {
			diagnostics: Renderers::default(),
			program,
			version: version.into(),
			entries: BTreeMap::new(),
			cli_routes: CliRoutes::default(),
		})
	}

	pub(crate) fn identity(&self) -> (&str, &str) {
		(&self.program, &self.version)
	}

	/// Replace the renderer used for framework diagnostics on the selected interface.
	pub fn diagnostic_renderer(
		&mut self,
		interface: Interface,
		renderer: crate::diagnostics::DiagnosticRenderer,
	) {
		*self.diagnostics.get_mut(interface) = renderer;
	}

	/// Validate an entry's namespace, bindings and interface registrations before adding its
	/// routes. An invalid or conflicting entry leaves the catalog unchanged.
	pub fn register<I, O, E, P>(
		&mut self,
		entry: Entry<I, O, E, P>,
	) -> Result<(), RegistrationError> {
		if entry.duplicate {
			return Err(RegistrationError(
				"interface registered more than once".into(),
			));
		}
		if entry.cli.is_none() && entry.rpc.is_none() && entry.mcp.is_none() {
			return Err(RegistrationError("entry has no enabled interface".into()));
		}

		let mut path = entry.namespace;
		path.push(entry.action.name().into());
		for segment in &path {
			validate_name(segment)?;
			if segment == "help" {
				return Err(RegistrationError("'help' is reserved".into()));
			}
		}
		if self.entries.keys().any(|other| conflicts(other, &path)) {
			return Err(RegistrationError(
				"duplicate action or namespace conflict".into(),
			));
		}

		let cli = entry
			.cli
			.map(|build| build(&self.program, &self.version))
			.transpose()?;
		let routes = self.cli_routes.prepare(&path, cli)?;

		let info = ActionInfo {
			path: path.clone(),
			description: entry.action.description().into(),
			input_schema: entry.action.input_schema().clone(),
		};
		let tool_info = entry.tool_description.map(|tool| ActionInfo {
			path: path.clone(),
			description: tool.description,
			input_schema: tool.input_schema,
		});

		self.entries.insert(
			path,
			Registered {
				info,
				tool_info,
				cli: !routes.is_empty(),
				rpc: entry.rpc,
				mcp: entry.mcp,
				mcp_observation: entry.mcp_observation,
			},
		);
		self.cli_routes.publish(routes);
		Ok(())
	}

	pub(crate) fn rpc_renderer(&self) -> crate::diagnostics::DiagnosticRenderer {
		self.diagnostics.rpc.clone()
	}

	pub(crate) fn mcp_tool(&self, path: &[String]) -> Option<McpTool<'_>> {
		let entry = self.entries.get(path)?;
		Some(McpTool {
			binding: entry.mcp.as_deref()?,
			observation: entry.mcp_observation,
			renderer: &self.diagnostics.mcp,
		})
	}

	/// List actions exposed on the requested interface, in command-path order.
	pub fn discover(&self, interface: Interface) -> Vec<&ActionInfo> {
		self.entries
			.values()
			.filter_map(|entry| match interface {
				Interface::Cli if entry.cli => Some(&entry.info),
				Interface::Rpc if entry.rpc.is_some() => Some(&entry.info),
				Interface::Mcp if entry.mcp.is_some() => {
					Some(entry.tool_info.as_ref().unwrap_or(&entry.info))
				}
				_ => None,
			})
			.collect()
	}

	/// Project the registered commands for help, host composition and completions.
	pub fn cli_command(&self) -> Command {
		self.cli_routes.command(&self.program, &self.version)
	}

	/// Parse native arguments including the executable name. Help and version requests return
	/// display text; an action match returns a prepared invocation.
	pub fn parse_cli(
		&self,
		arguments: impl IntoIterator<Item = impl Into<OsString>>,
	) -> Result<Parsed<PreparedCli>, CliError> {
		self.cli_routes.parse(
			&self.program,
			&self.version,
			arguments,
			self.diagnostics.cli.clone(),
		)
	}

	/// Run an RPC invocation synchronously in process and collect its structured presentation.
	pub fn invoke_rpc(
		&self,
		path: &[&str],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
	) -> Result<StructuredReport, InvocationError> {
		let invocation = self.prepare_rpc(path, input, observe, cancellation)?;
		Ok(smol::block_on(invocation.run()))
	}

	/// Run an MCP invocation synchronously in process and collect its structured presentation.
	pub fn invoke_mcp(
		&self,
		path: &[&str],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
	) -> Result<StructuredReport, InvocationError> {
		let invocation = self.prepare_mcp(path, input, observe, cancellation)?;
		Ok(smol::block_on(invocation.run()))
	}

	/// Decode RPC input and construct its view. Call [`PreparedStructured::run`] to execute it.
	pub fn prepare_rpc(
		&self,
		path: &[&str],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
	) -> Result<PreparedStructured, InvocationError> {
		self.prepare_structured(Interface::Rpc, path, input, observe, cancellation)
	}

	/// Decode the tool's advertised input and construct its MCP view.
	/// Call [`PreparedStructured::run`] to execute it.
	pub fn prepare_mcp(
		&self,
		path: &[&str],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
	) -> Result<PreparedStructured, InvocationError> {
		self.prepare_structured(Interface::Mcp, path, input, observe, cancellation)
	}

	fn prepare_structured(
		&self,
		interface: Interface,
		path: &[&str],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
	) -> Result<PreparedStructured, InvocationError> {
		let key: Vec<String> = path.iter().map(|s| (*s).into()).collect();
		let entry = self.entries.get(&key).ok_or(InvocationError::Unavailable)?;
		let prepare = match interface {
			Interface::Rpc => &entry.rpc,
			Interface::Mcp => &entry.mcp,
			Interface::Cli => unreachable!(),
		}
		.as_ref()
		.ok_or(InvocationError::Unavailable)?;

		(prepare.prepare)(
			Destination::Transcript,
			input,
			observe,
			cancellation,
			self.diagnostics.get(interface).clone(),
		)
	}

	pub(crate) fn prepare_rpc_live(
		&self,
		path: &[String],
		input: Value,
		observe: bool,
		cancellation: Cancellation,
		sink: Arc<dyn LiveSink>,
	) -> Result<PreparedStructured, InvocationError> {
		let prepare = self
			.entries
			.get(path)
			.and_then(|entry| entry.rpc.as_ref())
			.ok_or(InvocationError::Unavailable)?;

		(prepare.prepare)(
			Destination::Live(sink),
			input,
			observe,
			cancellation,
			self.diagnostics.rpc.clone(),
		)
	}
}

fn conflicts(a: &[String], b: &[String]) -> bool {
	a.starts_with(b) || b.starts_with(a)
}
