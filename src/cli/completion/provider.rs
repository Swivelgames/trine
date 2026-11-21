//! A field's domain suggestions, independent of shell transport.
use super::Context;
use clap::builder::ArgExt;
use std::{
	ffi::{OsStr, OsString},
	sync::Arc,
};

/// A shell inserts the value verbatim after applying its own quoting rules.
pub struct Candidate {
	pub value: OsString,
	pub description: Option<String>,
}

impl Candidate {
	pub fn new(value: impl Into<OsString>) -> Self {
		Self {
			value: value.into(),
			description: None,
		}
	}

	pub fn description(mut self, description: impl Into<String>) -> Self {
		self.description = Some(description.into());
		self
	}
}

type Suggest = dyn Fn(&Context, &OsStr) -> Vec<Candidate> + Send + Sync;

#[derive(Clone)]
pub(crate) struct Provider {
	suggest: Arc<Suggest>,
	pub(crate) group: Option<String>,
}

impl Provider {
	pub(crate) fn new(
		suggest: impl Fn(&Context, &OsStr) -> Vec<OsString> + Send + Sync + 'static,
	) -> Self {
		Self::candidates(move |context, current| {
			suggest(context, current)
				.into_iter()
				.map(Candidate::new)
				.collect()
		})
	}

	pub(crate) fn candidates(
		suggest: impl Fn(&Context, &OsStr) -> Vec<Candidate> + Send + Sync + 'static,
	) -> Self {
		Self {
			suggest: Arc::new(suggest),
			group: None,
		}
	}

	pub(crate) fn suggest(&self, context: &Context, current: &OsStr) -> Vec<Candidate> {
		(self.suggest)(context, current)
	}
}

impl std::fmt::Debug for Provider {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("Provider")
	}
}

impl ArgExt for Provider {}
