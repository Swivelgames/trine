//! Repeated-group children are useful only after their anchor has a complete value.
use super::Context;
use clap::{Arg, Command, builder::ArgExt};
use clap_lex::OsStrExt;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub(crate) struct GroupChild {
	pub(crate) anchor: String,
}

impl ArgExt for GroupChild {}

pub(super) fn excluded(command: &Command, context: &Context) -> BTreeSet<String> {
	let mut command = command;
	let mut completed = BTreeSet::new();
	let mut pending: Option<&Arg> = None;

	for word in context.arguments.iter().take(context.index).skip(1) {
		// A detached option value cannot also be a subcommand or group anchor.
		if let Some(option) = pending.take() {
			if !word.is_empty() && (word == "-" || !word.starts_with("-")) {
				completed.insert(option.get_id().as_str());
			}
			continue;
		}

		if word == "--" {
			break;
		}

		if let Some(long) = word.strip_prefix("--") {
			let (name, value) = long
				.split_once("=")
				.map_or((long, None), |(name, value)| (name, Some(value)));
			let option = command
				.get_arguments()
				.find(|argument| argument.get_long().is_some_and(|long| name == long));
			let Some(option) = option else { continue };

			if !option
				.get_num_args()
				.is_some_and(|arity| arity.takes_values())
			{
				continue;
			}

			if let Some(value) = value {
				if !value.is_empty() {
					completed.insert(option.get_id().as_str());
				}
			} else if !option.is_require_equals_set() {
				pending = Some(option);
			}
			continue;
		}

		if let Some(subcommand) = word.to_str().and_then(|name| command.find_subcommand(name)) {
			command = subcommand;
			completed.clear();
		}
	}

	command
		.get_arguments()
		.filter_map(|argument| {
			let child = argument.get::<GroupChild>()?;
			(!completed.contains(child.anchor.as_str()))
				.then(|| format!("arg::{}", argument.get_id()))
		})
		.collect()
}
