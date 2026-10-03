//! Statics in zeroed regions are declared only through
//! `oer_memory::zeroed_static!`.
//!
//! A `link_section` naming an input section the runtime linker script places
//! in a region the boot zeroes ([`oer_esp32s31_platform_layout::zeroed`]) is
//! refused anywhere in Rust source: the macro is the one place that writes
//! such an attribute, with the section as a macro argument rather than a
//! literal. The image linker stays the backstop for code tidy does not read,
//! vendor archives included.

use oer_esp32s31_platform_layout::zeroed::is_zeroed_input;

use crate::{Context, Result};

/// Every literal `link_section` that names a zeroed input section.
pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let mut problems = vec![];
    for file in context.repo.files().filter(|file| file.ends_with(".rs")) {
        let text = context.repo.read(file)?;
        for (index, line) in text.lines().enumerate() {
            if let Some(section) = link_section(line).filter(|section| is_zeroed_input(section)) {
                problems.push(format!(
                    "{file}:{}: `link_section = \"{section}\"` places a static in a zeroed region; \
                     declare it with `oer_memory::zeroed_static!`",
                    index + 1
                ));
            }
        }
    }
    Ok(problems)
}

/// The string literal of a `link_section = "..."` attribute on `line`.
fn link_section(line: &str) -> Option<&str> {
    let code = line.split("//").next()?;
    let rest = &code[code.find("link_section")? + "link_section".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let literal = rest.strip_prefix('"')?;
    Some(&literal[..literal.find('"')?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    const PACKAGE: (&str, &str) = ("p/Cargo.toml", "[package]\nname = \"p\"\n");

    fn found(source: &str) -> Vec<String> {
        problems(&[PACKAGE, ("p/src/lib.rs", source)], |context| {
            check(context).unwrap()
        })
    }

    #[test]
    fn a_literal_zeroed_section_is_refused() {
        let found = found(
            "#[unsafe(link_section = \".dma.bss.pool\")]\nstatic POOL: [u8; 4] = [0; 4];\n\
             #[link_section = \".psram.bss.x\"]\nstatic X: u8 = 0;\n",
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].starts_with("p/src/lib.rs:1: `link_section = \".dma.bss.pool\"`"));
        assert!(found[1].starts_with("p/src/lib.rs:3:"));
    }

    #[test]
    fn other_sections_the_macro_and_comments_pass() {
        let found = found(
            "#[unsafe(link_section = \".rwtext.isr\")]\nfn isr() {}\n\
             #[unsafe(link_section = \".psram.noinit.flags\")]\nstatic F: u8 = 0;\n\
             #[unsafe(link_section = $section)]\n\
             // an example: link_section = \".dma.bss.x\"\n\
             oer_memory::zeroed_static! { static P: u8 = zeroed in \".dma.bss.p\"; }\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }
}
