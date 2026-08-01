//! Concrete single-symbol execution command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut artifact = None;
    let mut companion = None;
    let mut symbol = None;
    let mut concrete_only = false;
    let mut print_timeline = false;
    let mut scenario = execution::Scenario::default();
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companion = Some(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol" => symbol = Some(take_value(&mut arguments, "--symbol")?),
            "--concrete-only" => concrete_only = true,
            "--timeline" => print_timeline = true,
            "--arg" => {
                let value = take_value(&mut arguments, "--arg")?;
                scenario
                    .arguments
                    .push(parse_u32(&value).ok_or("invalid --arg value")?);
            }
            "--mmio" => {
                let assignment = take_value(&mut arguments, "--mmio")?;
                let (address, value) = parse_assignment(&assignment, "--mmio")?;
                scenario.mmio_initial.insert(address, value);
            }
            "--read" => {
                let assignment = take_value(&mut arguments, "--read")?;
                let (address, value) = parse_assignment(&assignment, "--read")?;
                scenario
                    .mmio_reads
                    .entry(address)
                    .or_default()
                    .push_back(value);
            }
            "--ram" => {
                let assignment = take_value(&mut arguments, "--ram")?;
                let (address, value) = parse_assignment(&assignment, "--ram")?;
                seed_ram_word(&mut scenario, address, value);
            }
            "--observe" => {
                let assignment = take_value(&mut arguments, "--observe")?;
                let (address, length) = parse_assignment(&assignment, "--observe")?;
                observe_memory(&mut scenario, address, length)?;
            }
            "--max-steps" => {
                let value = take_value(&mut arguments, "--max-steps")?;
                scenario.max_steps = value.parse()?;
            }
            _ => return Err(format!("unknown execute option: {argument}").into()),
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let symbol = symbol.ok_or("missing --symbol")?;
    let mut image = execution::ExecutableImage::load(&artifact)?;
    if let Some(companion) = companion {
        image.add_companion(&companion)?;
    }
    let inventory = if concrete_only {
        execution::CoverageInventory::default()
    } else {
        image.coverage_inventory(&symbol)?
    };
    let result = execution::execute(&image, svd, &symbol, scenario)?;
    let unmapped: BTreeSet<_> = result
        .events
        .iter()
        .filter_map(unmapped_execution_address)
        .collect();
    for event in result.events {
        match event {
            execution::ExecutionEvent::Read {
                width,
                address,
                register,
                value,
            } => println!("EVENT\tR\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"),
            execution::ExecutionEvent::Write {
                width,
                address,
                register,
                value,
            } => println!("EVENT\tW\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}",),
            execution::ExecutionEvent::DelayMicros(micros) => {
                println!("EVENT\tDELAY\tmicros={micros}");
            }
            execution::ExecutionEvent::Fence {
                fm,
                predecessor,
                successor,
            } => println!("EVENT\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"),
        }
    }
    for call in &result.calls {
        println!("COVERED-CALL\t{call}");
    }
    if print_timeline {
        for (index, event) in result.timeline.iter().enumerate() {
            match event {
                execution::ExecutionTimelineEvent::Observable(event) => {
                    println!("TIMELINE-EVENT\t{index}\tOBSERVABLE\t{event:?}");
                }
                execution::ExecutionTimelineEvent::Call(call) => println!(
                    "TIMELINE-EVENT\t{index}\tCALL\t{}\t{}\targs={:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x}",
                    image.location(call.site),
                    call.symbol,
                    call.arguments[0],
                    call.arguments[1],
                    call.arguments[2],
                    call.arguments[3],
                    call.arguments[4],
                    call.arguments[5],
                    call.arguments[6],
                    call.arguments[7],
                ),
                execution::ExecutionTimelineEvent::Branch { site, taken } => println!(
                    "TIMELINE-EVENT\t{index}\tBRANCH\t{}\ttaken={taken}",
                    image.location(*site)
                ),
                execution::ExecutionTimelineEvent::RamRead {
                    width,
                    address,
                    value,
                } => println!(
                    "TIMELINE-EVENT\t{index}\tRAM-READ\t{width}\t{address:#010x}\tvalue={value:#010x}"
                ),
                execution::ExecutionTimelineEvent::RamWrite {
                    width,
                    address,
                    value,
                } => println!(
                    "TIMELINE-EVENT\t{index}\tRAM-WRITE\t{width}\t{address:#010x}\tvalue={value:#010x}"
                ),
            }
        }
    }
    let uncovered_branches = print_branch_coverage(
        "image",
        &image,
        &inventory.branch_outcomes,
        &result.branches,
    );
    for (address, edge) in &inventory.unresolved_edges {
        println!(
            "UNCOVERED-CONTROL-FLOW\timage\t{}\t{edge}",
            image.location(*address)
        );
    }
    for address in &unmapped {
        println!("UNCOVERED-MMIO\timage\t{address:#010x}");
    }
    for change in &result.memory_changes {
        println!(
            "MEMORY-CHANGE\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
            change.address, change.before, change.after
        );
    }
    println!(
        "RESULT\tsymbol={symbol}\tevidence={}\tsteps={}\treturn={:#010x}\tbranches={}\tbranch-events={}\tcalls={}\tcall-events={}\ttimeline-events={}\tmemory-changes={}\tuncovered-branch-outcomes={uncovered_branches}\tunresolved-control-flow={}\tunmapped-mmio={}",
        if concrete_only {
            "concrete-only"
        } else {
            "branch-complete"
        },
        result.steps,
        result.return_value,
        result.branches.len(),
        result.ordered_branches.len(),
        result.calls.len(),
        result.ordered_calls.len(),
        result.timeline.len(),
        result.memory_changes.len(),
        inventory.unresolved_edges.len(),
        unmapped.len(),
    );
    Ok(uncovered_branches == 0 && inventory.unresolved_edges.is_empty() && unmapped.is_empty())
}
