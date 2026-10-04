//! The stack policy of an image family (`stack.toml`, schema 5): the stacks
//! the gate bounds, each by the function that runs on it, the compiler move
//! limit and the runtime headroom the firmware's stack painting enforces.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;

/// The policy schema this gate reads.
pub const SCHEMA: u32 = 5;

/// A target's stack policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StackPolicy {
    pub schema: u32,
    /// The review of every linked function without a compiler frame record,
    /// relative to the policy file that names it.
    pub coverage_policy: PathBuf,
    /// The largest implicit copy or move the compiler admits
    /// (`-Z move-size-limit` with `-D large-assignments`).
    pub max_move_bytes: u64,
    /// The runtime's primary task stack.
    pub cpu0_task_stack: TaskStack,
    /// The runtime's secondary task stack, in images that start the second
    /// core.
    #[serde(default)]
    pub cpu1_task_stack: Option<TaskStack>,
    pub interrupt_stacks: InterruptStacks,
    /// The bootstrap image's only stack.
    pub bootstrap_stack: TaskStack,
}

/// One stack, the function that runs on it and the headroom it keeps.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskStack {
    /// The symbol of the function that runs on the stack from its top: the
    /// root of the call chains the gate bounds.
    pub root: String,
    /// The symbol whose size is the stack's capacity.
    #[serde(default)]
    pub storage_symbol: Option<String>,
    /// For a stack the linker script places as a range rather than an
    /// object: the symbols of its lowest address and of its top.
    #[serde(default)]
    pub bottom_symbol: Option<String>,
    #[serde(default)]
    pub top_symbol: Option<String>,
    /// Bytes the deepest call chain must leave unused; the runtime's stack
    /// painting holds every exercised workload to the same reserve.
    pub minimum_free_bytes: u32,
    /// Whether an image may link no storage for this stack (a single-core
    /// image's second task stack). An image that links the storage must
    /// contain the root.
    #[serde(default)]
    pub optional: bool,
}

/// Where a stack's capacity comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Storage<'a> {
    /// A sized symbol: the stack object.
    Symbol(&'a str),
    /// The linker script's bottom and top symbols.
    Range { bottom: &'a str, top: &'a str },
}

impl TaskStack {
    pub fn storage(&self) -> Storage<'_> {
        match (&self.storage_symbol, &self.bottom_symbol, &self.top_symbol) {
            (Some(symbol), None, None) => Storage::Symbol(symbol),
            (None, Some(bottom), Some(top)) => Storage::Range { bottom, top },
            _ => unreachable!("validated: one storage form"),
        }
    }

    fn validate(&self, name: &str) -> Result<()> {
        let form = match (&self.storage_symbol, &self.bottom_symbol, &self.top_symbol) {
            (Some(symbol), None, None) => !symbol.is_empty(),
            (None, Some(bottom), Some(top)) => !bottom.is_empty() && !top.is_empty(),
            _ => false,
        };
        if !form {
            return Err(format!(
                "`{name}` needs either `storage_symbol` or both `bottom_symbol` and `top_symbol`"
            )
            .into());
        }
        if self.root.is_empty() {
            return Err(format!("`{name}` names no root function").into());
        }
        if self.minimum_free_bytes == 0 {
            return Err(format!("`{name}` needs positive `minimum_free_bytes`").into());
        }
        Ok(())
    }
}

/// The dedicated SRAM interrupt stacks: their bound and margin are the
/// interrupt-stack gate's contract; this is the runtime painting's reserve.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterruptStacks {
    pub minimum_free_bytes: u32,
}

/// A policy that reuses a base policy (`extends`, relative to this file)
/// and adds the second core's task stack only its images start.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extension {
    schema: u32,
    extends: PathBuf,
    cpu1_task_stack: TaskStack,
}

impl StackPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let table: toml::Table =
            toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        let policy = if table.contains_key("extends") {
            let extension: Extension =
                toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
            let base_path = parent(path).join(&extension.extends);
            let base_source = std::fs::read_to_string(&base_path)
                .map_err(|error| format!("cannot read {}: {error}", base_path.display()))?;
            if toml::from_str::<toml::Table>(&base_source)
                .is_ok_and(|base| base.contains_key("extends"))
            {
                return Err(format!(
                    "{} extends {}, which itself extends another policy; extend the base directly",
                    path.display(),
                    base_path.display()
                )
                .into());
            }
            let mut base = Self::parse(&base_path, &base_source)?;
            if extension.schema != base.schema {
                return Err(format!(
                    "{} has schema {} but its base has schema {}",
                    path.display(),
                    extension.schema,
                    base.schema
                )
                .into());
            }
            if base.cpu1_task_stack.is_some() {
                return Err(format!(
                    "{} sets `cpu1_task_stack`, which its base already sets",
                    path.display()
                )
                .into());
            }
            base.cpu1_task_stack = Some(extension.cpu1_task_stack);
            base
        } else {
            Self::parse(path, &source)?
        };
        policy.validate()?;
        Ok(policy)
    }

    fn parse(path: &Path, source: &str) -> Result<Self> {
        let mut policy: Self =
            toml::from_str(source).map_err(|error| format!("{}: {error}", path.display()))?;
        policy.coverage_policy = parent(path).join(&policy.coverage_policy);
        Ok(policy)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(format!(
                "unsupported stack policy schema {}; expected {SCHEMA}",
                self.schema
            )
            .into());
        }
        if self.max_move_bytes == 0 || self.interrupt_stacks.minimum_free_bytes == 0 {
            return Err("the move limit and the interrupt stacks' reserve must be positive".into());
        }
        self.cpu0_task_stack.validate("cpu0_task_stack")?;
        if self.cpu0_task_stack.optional {
            return Err("`cpu0_task_stack` runs every runtime and cannot be optional".into());
        }
        if let Some(cpu1) = &self.cpu1_task_stack {
            cpu1.validate("cpu1_task_stack")?;
        }
        self.bootstrap_stack.validate("bootstrap_stack")?;
        if self.bootstrap_stack.optional {
            return Err(
                "`bootstrap_stack` is the bootstrap's only stack and cannot be optional".into(),
            );
        }
        Ok(())
    }

    /// The runtime's task stacks, by name.
    pub fn runtime_task_stacks(&self) -> Vec<(&'static str, &TaskStack)> {
        std::iter::once(("CPU0 task stack", &self.cpu0_task_stack))
            .chain(
                self.cpu1_task_stack
                    .iter()
                    .map(|stack| ("CPU1 task stack", stack)),
            )
            .collect()
    }

    /// The headroom reserves the runtime's stack painting enforces, as the
    /// build environment hands them to the firmware.
    pub fn runtime_environment(&self) -> Vec<(&'static str, String)> {
        let mut variables = vec![
            (
                "OPEN_RADIO_CPU0_STACK_MINIMUM_FREE_BYTES",
                self.cpu0_task_stack.minimum_free_bytes.to_string(),
            ),
            (
                "OPEN_RADIO_IRQ_STACK_MINIMUM_FREE_BYTES",
                self.interrupt_stacks.minimum_free_bytes.to_string(),
            ),
        ];
        if let Some(cpu1) = &self.cpu1_task_stack {
            variables.push((
                "OPEN_RADIO_CPU1_STACK_MINIMUM_FREE_BYTES",
                cpu1.minimum_free_bytes.to_string(),
            ));
        }
        variables
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}
