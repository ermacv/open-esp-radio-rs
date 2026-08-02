//! Direct trace comparison command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let split = filtered
        .iter()
        .position(|argument| argument == "--right-artifact")
        .ok_or("missing --right-artifact")?;
    let mut left_arguments = filtered[..split].iter().cloned();
    let mut right_arguments = filtered[split..].iter().cloned();
    let left = parse_input(&mut left_arguments, "left")?;
    let right = parse_input(&mut right_arguments, "right")?;
    let left_trace = extract(&left, svd)?;
    let right_trace = extract(&right, svd)?;
    print_trace(&left_trace);
    print_trace(&right_trace);
    if !left_trace.is_exact() || !right_trace.is_exact() {
        println!("VERDICT\tINCOMPLETE");
        return Ok(false);
    }
    let equal = traces_equal(&left_trace, &right_trace);
    println!("VERDICT\t{}", if equal { "MATCH" } else { "MISMATCH" });
    Ok(equal)
}
