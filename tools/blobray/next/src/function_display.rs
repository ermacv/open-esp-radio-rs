//! Streaming presentation of retained values. No analysis or hardware lookup.
use blobray_domain::*;
use std::io::{self, Write};
fn value(w: &mut dyn Write, v: &AbstractValue) -> io::Result<()> {
    match v {
        AbstractValue::Alternatives { values } => {
            write!(w, "one-of{{")?;
            for (index, alternative) in values.values().iter().enumerate() {
                if index > 0 {
                    write!(w, " | ")?;
                }
                // Nonrecursive leaves bound this call depth to two.
                value(w, &alternative.as_value())?;
            }
            write!(w, "}}")
        }
        AbstractValue::Expression { id } => write!(w, "v{id}"),
        AbstractValue::Unknown => write!(w, "unknown"),
        AbstractValue::Constant { value } => write!(w, "0x{value:08x}"),
        AbstractValue::ScopedAddress {
            source,
            object,
            address,
        } => write!(w, "{source:?}/{}:0x{address:08x}", object.artifact.as_str()),
        AbstractValue::ImageAddress { address } => write!(w, "image:0x{address:08x}"),
        AbstractValue::Section { section, offset } => write!(w, "section[{section}]{offset:+}"),
        AbstractValue::Symbol { symbol, addend } => write!(
            w,
            "symbol[{}:{}]{addend:+}",
            symbol.table_section, symbol.index
        ),
        AbstractValue::EntryStack { offset } => write!(w, "entry-sp{offset:+}"),
    }
}
pub(super) fn record(w: &mut dyn Write, r: &FunctionRecord) -> io::Result<()> {
    match r {
        FunctionRecord::MmioRange {
            offset,
            assertion,
            region,
        } => writeln!(
            w,
            "  @{offset:08x} MMIO region {} (review {})",
            region.name,
            assertion.as_str()
        ),
        FunctionRecord::Condition {
            offset,
            test,
            left,
            right,
        } => {
            write!(w, "  @{offset:08x} if ")?;
            value(w, left)?;
            write!(w, " {test:?} ")?;
            value(w, right)?;
            writeln!(w, "  // CFG alternatives; feasibility unproven")
        }
        FunctionRecord::Expression {
            id,
            offset,
            origin,
            expression,
        } => {
            write!(w, "  v{id} = ")?;
            match expression {
                Expression::EntryRegister { register } => write!(w, "entry.x{register}")?,
                Expression::Integer { op, left, right } => {
                    value(w, left)?;
                    write!(w, " {op:?} ")?;
                    value(w, right)?;
                }
                Expression::Load {
                    address,
                    width,
                    signed,
                } => {
                    write!(w, "read{}{}[", width * 8, if *signed { "s" } else { "u" })?;
                    value(w, address)?;
                    write!(w, "]")?;
                }
                Expression::CallResult { callsite, register } => {
                    write!(w, "call@{callsite:08x}.x{register}")?
                }
            }
            if *offset != u64::MAX {
                write!(
                    w,
                    "  // {}@{offset:08x}",
                    origin.as_ref().map_or("", |a| a.as_str())
                )?;
            }
            writeln!(w)
        }
        FunctionRecord::ReturnValue { offset, low, high } => {
            write!(w, "  @{offset:08x} return x10=")?;
            value(w, low)?;
            write!(w, ", x11=")?;
            value(w, high)?;
            writeln!(w)
        }
        FunctionRecord::Mmio {
            offset,
            assertion,
            register,
        } => writeln!(
            w,
            "  @{offset:08x} MMIO {} @0x{:08x} (review {})",
            register.name,
            register.address,
            assertion.as_str()
        ),
        FunctionRecord::CallResolution {
            offset,
            analysis,
            reason,
        } => writeln!(
            w,
            "  @{offset:08x} callee {} {}",
            analysis.as_ref().map_or("unresolved", |a| a.as_str()),
            reason
                .as_deref()
                .unwrap_or("retained may-effects; no execution claim")
        ),
        FunctionRecord::CalleeEffect {
            callsite,
            analysis,
            offset,
            access,
            width,
            address,
            value: v,
        } => {
            write!(
                w,
                "  call@{callsite:08x} may {access:?} {}-bit [",
                width * 8
            )?;
            value(w, address)?;
            write!(w, "]")?;
            if let Some(v) = v {
                write!(w, " <- ")?;
                value(w, v)?;
            }
            writeln!(w, "  // {} @{offset:08x}", analysis.as_str())
        }

        FunctionRecord::Transfer {
            offset,
            target,
            call,
        } => {
            write!(
                w,
                "  @{offset:08x} {} -> ",
                if *call { "call" } else { "jump/tail candidate" }
            )?;
            value(w, target)?;
            writeln!(w)
        }
        FunctionRecord::Instruction {
            offset,
            bytes,
            decoded,
        } => {
            write!(w, "{offset:08x}  ")?;
            for b in bytes {
                write!(w, "{b:02x}")?;
            }
            writeln!(w, "  {}", decoded.text)
        }
        FunctionRecord::Value {
            offset,
            register,
            value: v,
            relocation,
        } => {
            write!(w, "  @{offset:08x} x{register} = ")?;
            value(w, v)?;
            if let Some(r) = relocation {
                write!(w, " (relocation {}:{})", r.section, r.index)?;
            }
            writeln!(w)
        }
        FunctionRecord::MemoryAccess {
            offset,
            access,
            width,
            address,
            value: v,
            relocation,
        } => {
            write!(
                w,
                "  @{offset:08x} {access:?} {}-bit [",
                u16::from(*width) * 8
            )?;
            value(w, address)?;
            write!(w, "]")?;
            if let Some(v) = v {
                write!(w, " <- ")?;
                value(w, v)?;
            }
            if let Some(r) = relocation {
                write!(w, " (relocation {}:{})", r.section, r.index)?;
            }
            writeln!(w)
        }
        FunctionRecord::SemanticGap { offset, reason } => {
            writeln!(w, "  @{offset:08x} semantic gap: {reason:?}")
        }
        _ => {
            serde_json::to_writer(&mut *w, r)?;
            writeln!(w)
        }
    }
}
