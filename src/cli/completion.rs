//! Dynamic completion from the same command tree used for ordinary parsing.
mod bash_input;
mod context;
mod eligibility;
mod provider;
mod shell;

use clap::Command;
pub use clap_complete::Shell;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
pub use context::{Context, OptionValue};
pub(crate) use eligibility::GroupChild;
pub(crate) use provider::Provider;
use std::{ffi::OsString, io, path::Path, sync::Arc};

/// Complete partial input without preparing an action or constructing a view.
pub fn complete(command: Command, context: Context) -> io::Result<Vec<OsString>> {
	Ok(candidates(command, context, false)?
		.into_iter()
		.map(|candidate| candidate.get_value().to_owned())
		.collect())
}

fn candidates(
	command: Command,
	context: Context,
	group_domains_first: bool,
) -> io::Result<Vec<CompletionCandidate>> {
	if context.index >= context.arguments.len() {
		return Ok(Vec::new());
	}

	let context = Arc::new(context);
	let mut groups = Vec::new();
	let mut command = prepare(command, &context, &mut groups);
	command.build();
	let excluded = eligibility::excluded(&command, &context);
	let mut candidates = clap_complete::engine::complete(
		&mut command,
		context.arguments.clone(),
		context.index,
		Some(&context.cwd),
	)?;

	candidates.retain(|candidate| !candidate.get_id().is_some_and(|id| excluded.contains(id)));
	if group_domains_first {
		candidates.sort_by_key(|candidate| {
			!candidate
				.get_tag()
				.is_some_and(|tag| groups.contains(&tag.to_string()))
		});
	}

	Ok(candidates)
}

fn prepare(command: Command, context: &Arc<Context>, groups: &mut Vec<String>) -> Command {
	command
		.mut_args(|argument| {
			let Some(provider) = argument.get::<Provider>().cloned() else {
				return argument;
			};

			if let Some(group) = &provider.group {
				groups.push(group.clone());
			}

			let context = context.clone();
			argument.add(ArgValueCompleter::new(move |current: &std::ffi::OsStr| {
				provider
					.suggest(&context, current)
					.into_iter()
					.map(|value| {
						CompletionCandidate::new(value).tag(provider.group.clone().map(Into::into))
					})
					.collect()
			}))
		})
		.mut_subcommands(|command| prepare(command, context, groups))
}

/// Whether this is a shell's private query, before ordinary argument validation.
pub fn requested() -> bool {
	std::env::var_os(shell::VARIABLE).is_some()
}

/// Reply to an installed completion script. Errors are intentionally quiet at the host.
pub fn respond(command: Command, arguments: Vec<OsString>, cwd: &Path) -> io::Result<Vec<u8>> {
	shell::respond(command, arguments, cwd)
}

/// Generate registration only; shell configuration is never changed.
pub fn generate(command: &mut Command, shell: Shell) -> Vec<u8> {
	shell::registration(command.get_name(), shell)
}
