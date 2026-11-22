//! Terminal option selection on the invocation's joined view worker.
use super::{CliViewContext, prompt};
use crate::{Cancellation, invocation::Selection};
use inquire::{Select, error::InquireError, ui::Styled};
use std::io;

impl CliViewContext {
	/// Prompt for an option index on an interactive terminal. Cancellation or unavailable input
	/// returns `None`; terminal I/O failures propagate.
	pub fn select(
		&self,
		request: Selection,
		cancellation: &Cancellation,
	) -> io::Result<Option<usize>> {
		if cancellation.is_cancelled() || !self.can_prompt() {
			return Ok(None);
		}

		let mut style = prompt::style();
		style.highlighted_option_prefix = Styled::new("  >");
		let answer = Select::new(&request.message, request.options)
			.with_render_config(style)
			.without_help_message()
			.raw_prompt();

		match answer {
			Ok(option) => Ok(Some(option.index)),
			Err(InquireError::NotTTY) => Ok(None),
			Err(InquireError::OperationInterrupted | InquireError::OperationCanceled) => {
				cancellation.cancel();
				Ok(None)
			}
			Err(InquireError::IO(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
				cancellation.cancel();
				Ok(None)
			}
			Err(InquireError::IO(error)) => Err(error),
			Err(error) => Err(io::Error::other(error)),
		}
	}
}
