//! Direct trace extraction command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut input_arguments = filtered.into_iter();
    let input = parse_input(&mut input_arguments, "")?;
    let trace = extract(&input, svd)?;
    print_trace(&trace);
    Ok(trace.is_exact())
}
