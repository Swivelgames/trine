//! Multiple-choice interaction through a view independently of progress.

use std::convert::Infallible;
use trine::{
	Action, Cancellation, InvocationContext,
	view::{self, View},
};

#[test]
fn selection_uses_the_joined_view_without_subscribing_to_progress() {
	use trine::invocation::Selection;

	struct PickSecond;

	impl View<usize, (), ()> for PickSecond {
		type Completion = ();
		type Error = Infallible;

		async fn progress(&mut self, _: ()) -> Result<(), Infallible> {
			Ok(())
		}

		async fn select(
			&mut self,
			prompt: Selection,
			_: Cancellation,
		) -> Result<Option<usize>, Infallible> {
			assert_eq!(prompt.options, ["Keep", "Overwrite", "Cancel"]);
			Ok(Some(1))
		}

		async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), Infallible> {
			Ok(())
		}
	}

	let prompt = || Selection {
		message: "Choose files".into(),
		options: vec!["Keep".into(), "Overwrite".into(), "Cancel".into()],
	};
	let unattended = InvocationContext::<()>::unobserved(Cancellation::new());
	assert_eq!(smol::block_on(unattended.select(prompt())).unwrap(), None);

	let action = Action::new(
		"choose",
		"Choose files",
		move |_: (), context: InvocationContext<()>| async move {
			Ok::<_, ()>(context.select(prompt()).await.unwrap().unwrap())
		},
	)
	.unwrap();
	let report = view::run(&action, (), PickSecond, None, Cancellation::new());
	assert!(matches!(
		report.outcome,
		trine::execution::ExecutionOutcome::Completed(Ok(1))
	));
}
