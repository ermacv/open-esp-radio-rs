//! Comparison of concrete ordered observations; no execution or repository authority.
use blobray_domain::*;
pub const VERIFIER: &str = "ordered-mmio-fence-u32/model-3";
pub fn compare(
    left: &ExecutionObservation,
    right: &ExecutionObservation,
    compare_return: bool,
    control: &mut dyn RunControl,
) -> Result<CaseComparison> {
    let mut result = CaseComparison {
        verdict: ComparisonVerdict::Incomplete,
        event: None,
        return_difference: false,
    };
    for (index, (a, b)) in left.events.iter().zip(&right.events).enumerate() {
        control.checkpoint(1)?;
        if a != b {
            result.verdict = ComparisonVerdict::Diff;
            result.event = Some(index as u32);
            return Ok(result);
        }
    }
    let l = left.completed();
    let r = right.completed();
    // A finished shorter side cannot acquire the extra event observed on the other.
    if (l && left.events.len() < right.events.len())
        || (r && right.events.len() < left.events.len())
    {
        result.verdict = ComparisonVerdict::Diff;
        result.event = Some(left.events.len().min(right.events.len()) as u32);
        return Ok(result);
    }
    if !compare_return
        && l
        && r
        && std::mem::discriminant(&left.stop) == std::mem::discriminant(&right.stop)
    {
        result.verdict = ComparisonVerdict::Match;
        return Ok(result);
    }
    if !l || !r {
        return Ok(result);
    }
    if let (ExecutionStop::Returned { low: a, .. }, ExecutionStop::Returned { low: b, .. }) =
        (&left.stop, &right.stop)
    {
        if compare_return {
            match (a, b) {
                (Some(a), Some(b)) if a != b => {
                    result.verdict = ComparisonVerdict::Diff;
                    result.return_difference = true;
                    return Ok(result);
                }
                (Some(_), Some(_)) => {}
                _ => return Ok(result),
            }
        }
        result.verdict = ComparisonVerdict::Match;
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn observed(stop: ExecutionStop, events: Vec<ExecutionEvent>) -> ExecutionObservation {
        ExecutionObservation {
            stop,
            steps: 1,
            events,
            models: vec![],
        }
    }
    #[test]
    fn verdicts_respect_unknown_returns_and_incomplete_prefixes() {
        let done = observed(
            ExecutionStop::Returned {
                low: Some(7),
                high: None,
            },
            vec![],
        );
        let missing = observed(ExecutionStop::BlockedByPriorPhase, vec![]);
        assert_eq!(
            compare(&done, &done, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&done, &missing, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let more = observed(
            ExecutionStop::BlockedByPriorPhase,
            vec![ExecutionEvent::Write {
                address: 16,
                width: 4,
                value: 1,
            }],
        );
        assert_eq!(
            compare(&done, &more, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Diff
        );
        assert_eq!(
            compare(&missing, &more, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let unknown = observed(
            ExecutionStop::Returned {
                low: None,
                high: None,
            },
            vec![],
        );
        assert_eq!(
            compare(&unknown, &done, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
    }
}
