//! Regression test for issue #43: using metered with a *wrapped* error type.
//!
//! The aggregate `ErrorCount` works on any `Result` out of the box. An
//! `#[error_count]` breakdown classifies via `ClassifyError`, which the user
//! implements once for their wrapper to project to the inner enum.

use metered::CounterSource;
use metered_semantic::{measure, ClassifyError, ErrorCount};

#[metered_semantic::error_count(name = ErrCount, visibility = pub)]
pub enum MyError {
    Foo,
    Bar,
}

pub struct Wrapper {
    inner: MyError,
}

// Teach breakdowns how to classify our wrapped error.
impl<T> ClassifyError<MyError> for Result<T, Wrapper> {
    fn error_variant(&self) -> Option<&MyError> {
        self.as_ref().err().map(|w| &w.inner)
    }
}

fn do_work(
    breakdown: &ErrCount,
    errors: &ErrorCount,
    fail: Option<MyError>,
) -> Result<(), Wrapper> {
    measure!(
        breakdown,
        measure!(errors, {
            match fail {
                Some(e) => Err(Wrapper { inner: e }),
                None => Ok(()),
            }
        })
    )
}

#[test]
fn breakdown_and_aggregate_handle_wrapped_errors() {
    let breakdown: ErrCount = Default::default();
    let errors: ErrorCount = ErrorCount::default();

    let _ = do_work(&breakdown, &errors, None);
    let _ = do_work(&breakdown, &errors, Some(MyError::Foo));
    let _ = do_work(&breakdown, &errors, Some(MyError::Bar));
    let _ = do_work(&breakdown, &errors, Some(MyError::Foo));

    assert_eq!(
        errors.get(),
        3,
        "aggregate ErrorCount counts every wrapped Err"
    );
    assert_eq!(
        breakdown.foo.get(),
        2,
        "breakdown attributes Foo through ClassifyError"
    );
    assert_eq!(
        breakdown.bar.get(),
        1,
        "breakdown attributes Bar through ClassifyError"
    );
}
