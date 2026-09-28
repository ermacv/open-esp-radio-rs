//! Build HIL firmware image classes the way `cargo hil image build` does,
//! with the link-time stack, placement and application audits, and report
//! every class's outcome instead of stopping at the first failure.

use std::time::{Duration, Instant};

use oer_hil_runner_core::image::{ImageClass, Integration};

use crate::{Context, Result};

/// How far each class is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// The full image build and its audits.
    Build,
    /// A `cargo check` of the runtime with the class's features only.
    TypeCheck,
}

/// One class's result.
#[derive(Debug)]
pub struct Outcome {
    pub class: ImageClass,
    pub elapsed: Duration,
    pub failure: Option<String>,
}

/// The classes to take: `selected`, or every class when it is empty, in
/// [`ImageClass::ALL`] order without repeats.
pub fn classes(selected: &[ImageClass]) -> Vec<ImageClass> {
    ImageClass::ALL
        .into_iter()
        .filter(|class| selected.is_empty() || selected.contains(class))
        .collect()
}

/// Build (or type-check) each class one after the other in its shared
/// compile cache, then print one line per class.
pub fn run(ctx: &Context, selected: &[ImageClass], depth: Depth) -> Result<()> {
    let network = Integration::OwnedXarxa;
    let classes = classes(selected);
    let mut outcomes = Vec::with_capacity(classes.len());
    for (index, class) in classes.iter().copied().enumerate() {
        println!(
            "check firmware: [{}/{}] {} {}",
            index + 1,
            classes.len(),
            match depth {
                Depth::Build => "building",
                Depth::TypeCheck => "type-checking",
            },
            class.id()
        );
        let started = Instant::now();
        let result = match depth {
            Depth::Build => {
                oer_hil_runner_core::image::build(&ctx.root, class, network, None).map(|_| ())
            }
            Depth::TypeCheck => oer_hil_runner_core::image::check(&ctx.root, class, network),
        };
        outcomes.push(Outcome {
            class,
            elapsed: started.elapsed(),
            failure: result.err().map(|error| error.to_string()),
        });
    }
    print!("{}", summary(&outcomes));
    verdict(&outcomes)
}

/// One line per class, then the totals.
pub fn summary(outcomes: &[Outcome]) -> String {
    let mut text = String::new();
    for outcome in outcomes {
        let status = if outcome.failure.is_some() {
            "FAIL"
        } else {
            "PASS"
        };
        text.push_str(&format!(
            "{status} {:<36} {:>5}s",
            outcome.class.id(),
            outcome.elapsed.as_secs()
        ));
        if let Some(failure) = &outcome.failure {
            let first = failure.lines().next().unwrap_or_default();
            text.push_str(&format!("  {first}"));
        }
        text.push('\n');
    }
    let failed = outcomes.iter().filter(|o| o.failure.is_some()).count();
    text.push_str(&format!(
        "check firmware: {} passed, {failed} failed\n",
        outcomes.len() - failed
    ));
    text
}

/// Fails naming every failed class.
pub fn verdict(outcomes: &[Outcome]) -> Result<()> {
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_some())
        .map(|outcome| outcome.class.id())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("HIL firmware failed for {}", failed.join(", ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(class: ImageClass, failure: Option<&str>) -> Outcome {
        Outcome {
            class,
            elapsed: Duration::from_secs(3),
            failure: failure.map(str::to_owned),
        }
    }

    #[test]
    fn no_selection_takes_every_class() {
        assert_eq!(classes(&[]), ImageClass::ALL.to_vec());
    }

    #[test]
    fn selection_keeps_catalog_order_without_repeats() {
        let first = ImageClass::ALL[0];
        let last = ImageClass::ALL[ImageClass::ALL.len() - 1];
        assert_eq!(classes(&[last, first, last]), vec![first, last]);
    }

    #[test]
    fn a_failure_does_not_hide_the_other_classes() {
        let [first, second, third] = [ImageClass::ALL[0], ImageClass::ALL[1], ImageClass::ALL[2]];
        let outcomes = [
            outcome(first, None),
            outcome(second, Some("link failed\nmore detail")),
            outcome(third, Some("stack audit")),
        ];
        let text = summary(&outcomes);
        assert!(text.contains(&format!("PASS {}", first.id())));
        assert!(text.contains("link failed"));
        assert!(!text.contains("more detail"));
        assert!(text.contains("1 passed, 2 failed"));
        assert_eq!(
            verdict(&outcomes).unwrap_err().to_string(),
            format!("HIL firmware failed for {}, {}", second.id(), third.id())
        );
    }

    #[test]
    fn all_passing_classes_pass() {
        assert!(verdict(&[outcome(ImageClass::ALL[0], None)]).is_ok());
    }
}
