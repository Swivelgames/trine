//! Interpret partial argv once using the same field and group roles as CLI bindings.
use super::{
	Context,
	input::{Field, Group, Input, Value},
};
use clap::{Arg, Command};
use clap_lex::OsStrExt;
use std::ffi::OsStr;

pub(super) fn read(command: &Command, context: &Context) -> Input {
	let mut command = command;
	let mut input = enter(command);
	let mut pending: Option<(&Arg, usize)> = None;
	let mut escaped = false;
	let mut positional_index = 1;

	for (index, word) in context.arguments.iter().enumerate().skip(1) {
		if let Some((argument, start)) = pending.take() {
			let value = if index == context.index {
				Value::Incomplete
			} else if word.is_empty() || (word != "-" && word.starts_with("-")) {
				input.valid = false;
				Value::Ambiguous
			} else {
				Value::Present(word.clone())
			};

			record(&mut input, argument, start, value);
			continue;
		}

		if !escaped && word == "--" && index != context.index {
			escaped = true;
			continue;
		}

		if !escaped && let Some(long) = word.strip_prefix("--") {
			pending = option(command, &mut input, long, index, index == context.index);
			continue;
		}

		if !escaped && word != "-" && word.starts_with("-") {
			if index != context.index {
				input.valid = false;
			}
			continue;
		}

		if !escaped
			&& index != context.index
			&& let Some(next) = word.to_str().and_then(|name| command.find_subcommand(name))
		{
			command = next;
			input = enter(command);
			positional_index = 1;
			continue;
		}

		let Some(positional) = command
			.get_positionals()
			.find(|arg| arg.get_index() == Some(positional_index))
		else {
			if index != context.index {
				input.valid = false;
			}
			continue;
		};

		record(
			&mut input,
			positional,
			index,
			supplied(word, index == context.index),
		);
		if positional
			.get_num_args()
			.is_none_or(|arity| arity.max_values() <= 1)
		{
			positional_index += 1;
		}
	}

	if let Some((argument, index)) = pending {
		record(&mut input, argument, index, Value::Incomplete);
	}

	input.excluded = excluded(command, &input, context.index);

	input
}

fn option<'a>(
	command: &'a Command,
	input: &mut Input,
	long: &OsStr,
	index: usize,
	current: bool,
) -> Option<(&'a Arg, usize)> {
	let (name, inline) = long
		.split_once("=")
		.map_or((long, None), |(name, value)| (name, Some(value)));
	let argument = command
		.get_arguments()
		.find(|arg| arg.get_long().is_some_and(|long| name == long));
	let Some(argument) = argument else {
		if !current {
			input.valid = false;
		}
		return None;
	};

	if current && inline.is_none() {
		return None;
	}

	if let Some(value) = inline {
		record(input, argument, index, supplied(value, current));
		return None;
	}

	if argument
		.get_num_args()
		.is_some_and(|arity| arity.min_values() > 0)
		&& !argument.is_require_equals_set()
	{
		return Some((argument, index));
	}

	if argument.get::<Field>().is_some() {
		record(input, argument, index, Value::Present("true".into()));
	}

	None
}

fn excluded(command: &Command, input: &Input, cursor: usize) -> std::collections::BTreeSet<String> {
	command
		.get_arguments()
		.filter_map(|argument| {
			let field = argument.get::<Field>()?;
			let group = field.group.as_ref().filter(|group| !group.anchor)?;
			let open = input
				.groups
				.get(&group.name)
				.and_then(|rows| rows.iter().rev().find(|row| row.anchor_index < cursor));
			let available = open
				.is_some_and(|row| matches!(row.fields.get(&row.anchor), Some(Value::Present(_))));

			(!available).then(|| format!("arg::{}", argument.get_id()))
		})
		.collect()
}

fn enter(command: &Command) -> Input {
	let mut input = Input {
		valid: true,
		..Input::default()
	};

	for argument in command.get_arguments() {
		let Some(field) = argument.get::<Field>() else {
			continue;
		};

		if let Some(long) = argument.get_long() {
			input.options.insert(long.into(), field.name.clone());
		}
		if field.repeated {
			input.lists.insert(field.name.clone(), Vec::new());
		}
		if let Some(group) = &field.group {
			input.groups.entry(group.name.clone()).or_default();
		}
		if let Some(value) = &field.assigned {
			input
				.fields
				.insert(field.name.clone(), Value::Present(value.clone()));
		}
	}

	input
}

fn supplied(value: &OsStr, current: bool) -> Value {
	if current {
		Value::Incomplete
	} else if value.is_empty() {
		Value::Ambiguous
	} else {
		Value::Present(value.to_owned())
	}
}

fn record(input: &mut Input, argument: &Arg, index: usize, value: Value) {
	let Some(field) = argument.get::<Field>() else {
		return;
	};

	if field.repeated {
		match &value {
			Value::Present(word) => input
				.lists
				.get_mut(&field.name)
				.expect("registered list")
				.push(word.clone()),
			Value::Incomplete => {}
			Value::Ambiguous => input.valid = false,
		}
		input.fields.insert(field.name.clone(), value);
		return;
	}

	let first_assignment = field.assigned.is_some() && input.explicit.insert(field.name.clone());
	let (fields, name) = if let Some(group) = &field.group {
		let rows = input.groups.entry(group.name.clone()).or_default();
		if group.anchor {
			rows.push(Group {
				anchor_index: index,
				anchor: group.child.clone(),
				..Group::default()
			});
		}

		let Some(row) = rows.last_mut() else {
			input.valid = false;
			return;
		};
		(&mut row.fields, &group.child)
	} else {
		(&mut input.fields, &field.name)
	};

	let value = if let Some(existing) = fields.get(name) {
		match (existing, &value) {
			(Value::Present(old), Value::Present(new)) if first_assignment && old == new => value,
			_ => Value::Ambiguous,
		}
	} else {
		value
	};
	fields.insert(name.clone(), value);
}
