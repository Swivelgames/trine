//! A field's domain suggestions, independent of shell transport.
use super::Context;
use clap::builder::ArgExt;
use std::{
	ffi::{OsStr, OsString},
	sync::Arc,
};

type Suggest = dyn Fn(&Context, &OsStr) -> Vec<OsString> + Send + Sync;

#[derive(Clone)]
pub(crate) struct Provider {
	suggest: Arc<Suggest>,
	pub(crate) group: Option<String>,
}

impl Provider {
	pub(crate) fn new(
		suggest: impl Fn(&Context, &OsStr) -> Vec<OsString> + Send + Sync + 'static,
	) -> Self {
		Self {
			suggest: Arc::new(suggest),
			group: None,
		}
	}

	pub(crate) fn suggest(&self, context: &Context, current: &OsStr) -> Vec<OsString> {
		(self.suggest)(context, current)
	}
}

impl std::fmt::Debug for Provider {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("Provider")
	}
}

impl ArgExt for Provider {}
