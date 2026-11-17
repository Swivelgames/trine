//! Binding-derived partial values. No action input is admitted or deserialized here.
use super::OptionValue;
use clap::builder::ArgExt;
use std::{
	collections::{BTreeMap, BTreeSet},
	ffi::OsString,
};

#[derive(Clone, Debug)]
pub(crate) struct Field {
	pub(crate) name: String,
	pub(crate) group: Option<GroupField>,
	pub(crate) repeated: bool,
	pub(crate) assigned: Option<OsString>,
}

#[derive(Clone, Debug)]
pub(crate) struct GroupField {
	pub(crate) name: String,
	pub(crate) child: String,
	pub(crate) anchor: bool,
}

impl ArgExt for Field {}

#[derive(Clone, Debug)]
pub(super) enum Value {
	Present(OsString),
	Incomplete,
	Ambiguous,
}

/// One explicitly supplied repeated group, retaining native field values.
#[derive(Clone, Debug, Default)]
pub struct Group {
	pub(super) fields: BTreeMap<String, Value>,
	pub(super) anchor_index: usize,
	pub(super) anchor: String,
}

impl Group {
	pub fn field(&self, name: &str) -> OptionValue<'_> {
		value(self.fields.get(name))
	}
}

#[derive(Clone, Debug, Default)]
pub(super) struct Input {
	pub(super) fields: BTreeMap<String, Value>,
	pub(super) groups: BTreeMap<String, Vec<Group>>,
	pub(super) options: BTreeMap<String, String>,
	pub(super) excluded: BTreeSet<String>,
	pub(super) explicit: BTreeSet<String>,
	pub(super) valid: bool,
}

impl Input {
	pub(super) fn field(&self, name: &str) -> OptionValue<'_> {
		if !self.valid {
			return OptionValue::Ambiguous;
		}

		value(self.fields.get(name))
	}
}

pub(super) fn value(value: Option<&Value>) -> OptionValue<'_> {
	match value {
		None => OptionValue::Absent,
		Some(Value::Present(value)) => OptionValue::Present(value),
		Some(Value::Incomplete) => OptionValue::Incomplete,
		Some(Value::Ambiguous) => OptionValue::Ambiguous,
	}
}
