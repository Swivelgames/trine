//! Optional terminal confirmation on the joined presentation worker.
use super::CliViewContext;
use crate::{Cancellation, invocation::Confirmation};
use inquire::{
	Confirm,
	error::InquireError,
	ui::{RenderConfig, Styled},
};
use std::io::{self, IsTerminal};

impl CliViewContext {
	/// Prompt only when both input and the presentation stream are terminals.
	///
	/// Inquire handles keyboard Ctrl+C and restores terminal modes before returning.
	/// A synchronous prompt cannot be interrupted by an external cancellation token;
	/// it stays on the joined view worker, without an abandoned stdin reader thread.
	pub fn confirm(
		&self,
		request: &Confirmation,
		cancellation: &Cancellation,
	) -> io::Result<Option<bool>> {
		if cancellation.is_cancelled() {
			return Ok(Some(false));
		}
		if !self.stderr.is_terminal() || !io::stdin().is_terminal() || !io::stderr().is_terminal() {
			return Ok(None);
		}

		// Inquire adds one separator space after each prefix: three plain spaces
		// yield a four-space inset without its default question/answer ornaments.
		let mut render = RenderConfig::empty();
		render.prompt_prefix = Styled::new("   ");
		render.answered_prompt_prefix = Styled::new("   ");
		render.error_message.prefix = Styled::new("   ");
		render.canceled_prompt_indicator = Styled::new("cancelled");
		let answer = Confirm::new(&request.message)
			.with_default(false)
			.with_render_config(render)
			.prompt();
		map_answer(answer, cancellation)
	}
}

fn map_answer(
	answer: Result<bool, InquireError>,
	cancellation: &Cancellation,
) -> io::Result<Option<bool>> {
	match answer {
		Ok(answer) => Ok(Some(answer && !cancellation.is_cancelled())),
		Err(InquireError::OperationInterrupted) => {
			cancellation.cancel();
			Ok(Some(false))
		}
		Err(InquireError::OperationCanceled) => Ok(Some(false)),
		Err(InquireError::NotTTY) => Ok(None),
		Err(InquireError::IO(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
			Ok(Some(false))
		}
		Err(InquireError::IO(error)) => Err(error),
		Err(error) => Err(io::Error::other(error)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn interruptions_are_cancellation_not_presentation_failure() {
		let cancellation = Cancellation::new();

		let answer = map_answer(Err(InquireError::OperationInterrupted), &cancellation).unwrap();

		assert_eq!(answer, Some(false));
		assert!(cancellation.is_cancelled());
	}

	#[test]
	fn escape_and_eof_never_grant_consent() {
		let cancellation = Cancellation::new();

		for error in [
			InquireError::OperationCanceled,
			InquireError::IO(io::Error::from(io::ErrorKind::UnexpectedEof)),
		] {
			let answer = map_answer(Err(error), &cancellation).unwrap();
			assert_eq!(answer, Some(false));
		}

		assert!(!cancellation.is_cancelled());
	}
}
