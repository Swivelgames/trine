#![deny(missing_docs)]
#![doc = include_str!("doc_links.md")]
#![doc = include_str!("../README.md")]

pub mod action;
pub mod application;
pub mod cli;
pub mod diagnostics;
pub mod error;
pub mod execution;
pub mod invocation;
mod jsonrpc;
pub mod mcp;
pub mod native_path;
pub mod registry;
pub mod rpc;
pub mod structured;
pub mod view;

pub use action::{Action, RegistrationError};
pub use application::{Application, Entry};
pub use invocation::{Cancellation, EmitError, InvocationContext};
pub use native_path::NativePath;

// GitHub resolves each page's relative Markdown links against the checkout.
// Rustdoc uses the first definition of each label to select its local module or
// the corresponding example in this version of the published crate.
macro_rules! doc_page {
	($page:ident, $markdown:literal, $( $label:literal => $example:literal ),+) => {
		#[doc = include_str!("doc_links.md")]
		#[doc = concat!($( "[", $label, "]: https://docs.rs/crate/trine/",
			env!("CARGO_PKG_VERSION"), "/source/examples/", $example, "\n", )+ "\n")]
		#[doc = include_str!($markdown)]
		pub mod $page {}
	};
}

/// Hands-on tutorials using the runnable examples in the Trine checkout.
pub mod tutorials {
	doc_page!(first_action, "../docs/tutorials/first-action.md", "hello" => "hello.rs");
	doc_page!(directory_scan, "../docs/tutorials/directory-scan.md", "scan-example" => "directory_scan.rs");
	doc_page!(interactive_decisions, "../docs/tutorials/interactive-decisions.md", "decisions" => "decisions.rs");
}

/// Guides to designing and integrating Trine applications.
pub mod guides {
	doc_page!(cli_inputs, "../docs/guides/cli-inputs.md",
		"repo-plan" => "repo-plan/main.rs", "remotes" => "repo-plan/remotes.rs");
	doc_page!(results_and_failures, "../docs/guides/results-and-failures.md", "hello" => "hello.rs");
	doc_page!(hosting_and_embedding, "../docs/guides/hosting-and-embedding.md", "hello" => "hello.rs");
}
