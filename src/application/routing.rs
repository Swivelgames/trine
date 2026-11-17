//! CLI aliases and namespaces projected from the shared action catalog.

use super::{CliPrepare, CliRegistration, PreparedCli, conflicts};
use crate::{
	RegistrationError,
	cli::{CliError, Parsed},
	diagnostics::DiagnosticRenderer,
};
use clap::{Arg, ArgAction, Command, error::ErrorKind};
use std::{
	collections::{BTreeMap, BTreeSet},
	ffi::OsString,
	sync::Arc,
};

pub(super) struct CliRoute {
	command: Command,
	original: String,
	prepare: Arc<CliPrepare>,
}

#[derive(Default)]
pub(super) struct CliRoutes {
	routes: BTreeMap<Vec<String>, CliRoute>,
}

impl CliRoutes {
	/// Stage all aliases before publication so a conflict leaves the catalog intact.
	pub(super) fn prepare(
		&self,
		path: &[String],
		cli: Option<CliRegistration>,
	) -> Result<Vec<(Vec<String>, CliRoute)>, RegistrationError> {
		let Some(cli) = cli else {
			return Ok(Vec::new());
		};

		Self::validate_json_option(&cli)?;

		let mut routes = Vec::new();
		for command in cli.commands {
			let original = command.get_name().to_owned();
			let mut route = path.to_vec();
			*route.last_mut().unwrap() = original.clone();

			if self
				.routes
				.keys()
				.chain(routes.iter().map(|(path, _)| path))
				.any(|other| conflicts(other, &route))
			{
				return Err(RegistrationError(format!(
					"duplicate command, alias or namespace: {}",
					route.join(" ")
				)));
			}

			routes.push((
				route,
				CliRoute {
					command,
					original,
					prepare: cli.prepare.clone(),
				},
			));
		}

		Ok(routes)
	}

	fn validate_json_option(cli: &CliRegistration) -> Result<(), RegistrationError> {
		let has_collision = cli.commands.iter().any(|command| {
			command
				.get_arguments()
				.any(|argument| argument.get_long() == Some("json"))
		});
		if has_collision {
			return Err(RegistrationError(
				"--json is reserved for CLI JSON presentation".into(),
			));
		}
		Ok(())
	}

	pub(super) fn publish(&mut self, routes: Vec<(Vec<String>, CliRoute)>) {
		self.routes.extend(routes);
	}

	pub(super) fn parse(
		&self,
		program: &str,
		version: &str,
		arguments: impl IntoIterator<Item = impl Into<OsString>>,
		renderer: DiagnosticRenderer,
	) -> Result<Parsed<PreparedCli>, CliError> {
		let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
		let command = self.command(program, version);
		let matches = match command.try_get_matches_from(arguments.clone()) {
			Ok(matches) => matches,
			Err(error)
				if matches!(
					error.kind(),
					ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
				) =>
			{
				return Ok(Parsed::Display(error.to_string()));
			}
			Err(error) => return Err(CliError(error.to_string())),
		};

		let json = matches.get_flag("trine:json");
		let mut path = Vec::new();
		let mut cursor = &matches;
		while let Some((name, matches)) = cursor.subcommand() {
			path.push(name.to_owned());
			cursor = matches;
		}

		let route = self
			.routes
			.get(&path)
			.ok_or_else(|| CliError("choose an action".into()))?;

		let arguments = crate::cli::output_mode::without_json(arguments);
		let normalized = [OsString::from(program), OsString::from(&route.original)]
			.into_iter()
			.chain(arguments.into_iter().skip(path.len() + 1))
			.collect();

		(route.prepare)(normalized).map(|parsed| match parsed {
			Parsed::Input(mut prepared) => {
				prepared.renderer = renderer;
				prepared.json = json;
				Parsed::Input(prepared)
			}
			Parsed::Display(text) => Parsed::Display(text),
		})
	}

	pub(super) fn command(&self, program: &str, version: &str) -> Command {
		let command = self.command_tree(program, &[]).version(version.to_owned());

		command.arg(
			Arg::new("trine:json")
				.long("json")
				.global(true)
				.action(ArgAction::SetTrue)
				.help("Print one structured JSON result"),
		)
	}

	fn command_tree(&self, program: &str, prefix: &[String]) -> Command {
		if let Some(route) = self.routes.get(prefix) {
			return route.command.clone();
		}

		let mut command = Command::new(
			prefix
				.last()
				.map(String::as_str)
				.unwrap_or(program)
				.to_owned(),
		)
		.subcommand_required(true);

		let children: BTreeSet<_> = self
			.routes
			.keys()
			.filter(|path| path.starts_with(prefix))
			.filter_map(|path| path.get(prefix.len()))
			.collect();

		for child in children {
			let mut path = prefix.to_vec();
			path.push(child.clone());
			command = command.subcommand(self.command_tree(program, &path));
		}

		command
	}
}
