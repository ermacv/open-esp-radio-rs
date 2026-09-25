//! Single-point mutations of production source, limited to executed code.
//!
//! A mutation run asks whether the typed scenarios notice a defect in the
//! compiled production code they compare. Candidate mutants come from a fixed
//! set of operators applied only where the scenarios' retained executions
//! reached production code: debug line information of the probe ELF maps the
//! executed instructions to source lines, and each mutant records which
//! scenarios reach its line.
use crate::harness::{Result, invalid};
use proc_macro2::LineColumn;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// A source line of one file, relative to the repository root.
pub type SourceLine = (PathBuf, u32);

/// Source lines within `scope` of every instruction in `pcs`, including the
/// lines of inlined frames, with the scenarios that reached each.
pub fn executed_lines(
    elf: &Path,
    reach: &BTreeMap<String, BTreeSet<u32>>,
    root: &Path,
    scope: &Path,
) -> Result<BTreeMap<SourceLine, BTreeSet<String>>> {
    use object::{Object, ObjectSection};
    let bytes = std::fs::read(elf)?;
    let file = object::File::parse(&*bytes)?;
    let endian = gimli::RunTimeEndian::Little;
    let dwarf = gimli::Dwarf::load(|id| {
        let data = file
            .section_by_name(id.name())
            .and_then(|section| section.data().ok())
            .unwrap_or(&[]);
        Ok::<_, gimli::Error>(gimli::EndianSlice::new(data, endian))
    })?;
    let context = addr2line::Context::from_dwarf(dwarf)?;
    let scope = root.join(scope).canonicalize()?;
    let mut lines: BTreeMap<SourceLine, BTreeSet<String>> = BTreeMap::new();
    let mut cache: BTreeMap<u32, Vec<SourceLine>> = BTreeMap::new();
    for (suite, pcs) in reach {
        for pc in pcs {
            if !cache.contains_key(pc) {
                let mut located = vec![];
                let mut frames = context
                    .find_frames(u64::from(*pc))
                    .skip_all_loads()
                    .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?;
                while let Some(frame) = frames
                    .next()
                    .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?
                {
                    let Some(location) = frame.location else {
                        continue;
                    };
                    let (Some(path), Some(line)) = (location.file, location.line) else {
                        continue;
                    };
                    let path = Path::new(path);
                    if let Ok(relative) = path.strip_prefix(&scope) {
                        located.push((
                            scope.strip_prefix(root.canonicalize()?)?.join(relative),
                            line,
                        ));
                    }
                }
                cache.insert(*pc, located);
            }
            for line in &cache[pc] {
                lines.entry(line.clone()).or_default().insert(suite.clone());
            }
        }
    }
    Ok(lines)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operator {
    /// An integer literal, or a scalar integer constant, plus one.
    Constant,
    /// A comparison replaced by its boundary neighbor or negation.
    Comparison,
    /// A branch condition negated.
    Guard,
    /// `min` and `max` exchanged.
    Bound,
    /// Two adjacent register writes exchanged.
    Order,
}

/// One single-point change: bytes `start..end` of `file` become `replacement`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Mutant {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
    pub operator: Operator,
    pub start: usize,
    pub end: usize,
    pub original: String,
    pub replacement: String,
}

impl Mutant {
    pub fn id(&self) -> String {
        format!(
            "{}:{}:{}:{:?}",
            self.file.display(),
            self.line,
            self.column,
            self.operator
        )
    }
    pub fn apply(&self, source: &str) -> String {
        format!(
            "{}{}{}",
            &source[..self.start],
            self.replacement,
            &source[self.end..]
        )
    }
}

/// Byte offsets of each line start.
struct Lines<'s> {
    source: &'s str,
    starts: Vec<usize>,
}
impl<'s> Lines<'s> {
    fn new(source: &'s str) -> Self {
        let starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { source, starts }
    }
    /// Byte offset of a 1-based line and 0-based character column.
    fn offset(&self, at: LineColumn) -> usize {
        let start = self.starts[at.line - 1];
        start
            + self.source[start..]
                .char_indices()
                .nth(at.column)
                .map_or(self.source.len() - start, |(i, _)| i)
    }
}

struct Generator<'s> {
    file: PathBuf,
    lines: Lines<'s>,
    executed: &'s BTreeSet<u32>,
    mutants: Vec<Mutant>,
}

const INTEGER_TYPES: [&str; 12] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
];

fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test"))
    })
}

impl Generator<'_> {
    fn push(&mut self, span: proc_macro2::Span, operator: Operator, replacement: String) {
        let (start, end) = (
            self.lines.offset(span.start()),
            self.lines.offset(span.end()),
        );
        let original = self.lines.source[start..end].to_owned();
        if original == replacement {
            return;
        }
        self.mutants.push(Mutant {
            file: self.file.clone(),
            line: span.start().line as u32,
            column: span.start().column as u32 + 1,
            operator,
            start,
            end,
            original,
            replacement,
        });
    }
    fn executed(&self, span: proc_macro2::Span) -> bool {
        self.executed.contains(&(span.start().line as u32))
    }
    fn plus_one(literal: &syn::LitInt) -> Option<String> {
        let value: u128 = literal.base10_parse().ok()?;
        Some(format!("{}{}", value.checked_add(1)?, literal.suffix()))
    }
    fn is_register_write(statement: &syn::Stmt) -> bool {
        let syn::Stmt::Expr(expression, Some(_)) = statement else {
            return false;
        };
        let syn::Expr::MethodCall(call) = expression else {
            return false;
        };
        matches!(
            call.method.to_string().as_str(),
            "write" | "modify" | "write_with_zero"
        )
    }
}

impl<'ast> Visit<'ast> for Generator<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        // Scalar constants apply wherever the file's code executed.
        if let (
            syn::Type::Path(path),
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(literal),
                ..
            }),
        ) = (&*item.ty, &*item.expr)
            && path
                .path
                .get_ident()
                .is_some_and(|i| INTEGER_TYPES.contains(&i.to_string().as_str()))
            && !self.executed.is_empty()
            && let Some(replacement) = Self::plus_one(literal)
        {
            self.push(literal.span(), Operator::Constant, replacement);
        }
    }
    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if let (
            syn::Type::Path(path),
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(literal),
                ..
            }),
        ) = (&item.ty, &item.expr)
            && path
                .path
                .get_ident()
                .is_some_and(|i| INTEGER_TYPES.contains(&i.to_string().as_str()))
            && !self.executed.is_empty()
            && let Some(replacement) = Self::plus_one(literal)
        {
            self.push(literal.span(), Operator::Constant, replacement);
        }
    }
    fn visit_expr_lit(&mut self, expression: &'ast syn::ExprLit) {
        if let syn::Lit::Int(literal) = &expression.lit
            && self.executed(literal.span())
            && let Some(replacement) = Self::plus_one(literal)
        {
            self.push(literal.span(), Operator::Constant, replacement);
        }
    }
    fn visit_expr_binary(&mut self, expression: &'ast syn::ExprBinary) {
        use syn::BinOp::*;
        let replacement = match expression.op {
            Lt(_) => Some("<="),
            Le(_) => Some("<"),
            Gt(_) => Some(">="),
            Ge(_) => Some(">"),
            Eq(_) => Some("!="),
            Ne(_) => Some("=="),
            _ => None,
        };
        if let Some(replacement) = replacement
            && self.executed(expression.op.span())
        {
            self.push(
                expression.op.span(),
                Operator::Comparison,
                replacement.into(),
            );
        }
        syn::visit::visit_expr_binary(self, expression);
    }
    fn visit_expr_if(&mut self, expression: &'ast syn::ExprIf) {
        if !matches!(*expression.cond, syn::Expr::Let(_)) && self.executed(expression.cond.span()) {
            let span = expression.cond.span();
            let (start, end) = (
                self.lines.offset(span.start()),
                self.lines.offset(span.end()),
            );
            let condition = self.lines.source[start..end].to_owned();
            self.push(span, Operator::Guard, format!("!({condition})"));
        }
        syn::visit::visit_expr_if(self, expression);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let swapped = match call.method.to_string().as_str() {
            "min" => Some("max"),
            "max" => Some("min"),
            _ => None,
        };
        if let Some(swapped) = swapped
            && self.executed(call.method.span())
        {
            self.push(call.method.span(), Operator::Bound, swapped.into());
        }
        syn::visit::visit_expr_method_call(self, call);
    }
    fn visit_block(&mut self, block: &'ast syn::Block) {
        for pair in block.stmts.windows(2) {
            if Self::is_register_write(&pair[0])
                && Self::is_register_write(&pair[1])
                && self.executed(pair[0].span())
                && self.executed(pair[1].span())
            {
                let (a, b) = (pair[0].span(), pair[1].span());
                let (a_start, a_end) = (self.lines.offset(a.start()), self.lines.offset(a.end()));
                let (b_start, b_end) = (self.lines.offset(b.start()), self.lines.offset(b.end()));
                let source = self.lines.source;
                let replacement = format!(
                    "{}{}{}",
                    &source[b_start..b_end],
                    &source[a_end..b_start],
                    &source[a_start..a_end]
                );
                self.push(a.join(b).unwrap_or(a), Operator::Order, replacement);
            }
        }
        syn::visit::visit_block(self, block);
    }
}

/// Every mutant of `source` on an executed line, plus scalar constants of a
/// file with any executed line, ascending by position.
pub fn generate(file: &Path, source: &str, executed: &BTreeSet<u32>) -> Result<Vec<Mutant>> {
    let syntax =
        syn::parse_file(source).map_err(|e| invalid(format!("{}: {e}", file.display())))?;
    let mut generator = Generator {
        file: file.to_owned(),
        lines: Lines::new(source),
        executed,
        mutants: vec![],
    };
    generator.visit_file(&syntax);
    let mut mutants = generator.mutants;
    mutants.sort_by_key(|m| (m.start, m.end, m.operator));
    mutants.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.replacement == b.replacement);
    Ok(mutants)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
const LIMIT: u32 = 7;
fn step(value: u32, bus: &Bus) -> u32 {
    if value < LIMIT {
        bus.a.write(|w| w.bits(1));
        bus.b.modify(|_, w| w.bits(2));
    }
    value.min(3)
}
fn cold(value: u32) -> bool {
    value == 9
}
#[cfg(test)]
mod tests {
    fn check() -> bool { 1 < 2 }
}
"#;

    fn mutants(executed: &[u32]) -> Vec<(u32, Operator, String, String)> {
        generate(
            Path::new("phy.rs"),
            SOURCE,
            &executed.iter().copied().collect(),
        )
        .unwrap()
        .into_iter()
        .map(|m| (m.line, m.operator, m.original, m.replacement))
        .collect()
    }

    #[test]
    fn mutants_cover_every_operator_on_executed_lines_only() {
        let all = mutants(&[4, 5, 6, 8]);
        let ops: BTreeSet<_> = all.iter().map(|m| m.1).collect();
        assert_eq!(
            ops,
            BTreeSet::from([
                Operator::Constant,
                Operator::Comparison,
                Operator::Guard,
                Operator::Bound,
                Operator::Order
            ])
        );
        assert!(all.contains(&(2, Operator::Constant, "7".into(), "8".into())));
        assert!(all.contains(&(4, Operator::Comparison, "<".into(), "<=".into())));
        assert!(all.contains(&(
            4,
            Operator::Guard,
            "value < LIMIT".into(),
            "!(value < LIMIT)".into()
        )));
        assert!(all.contains(&(8, Operator::Bound, "min".into(), "max".into())));
        // The unexecuted function and test code produce no mutants.
        assert!(all.iter().all(|m| m.0 != 11 && m.0 < 13));
        // Without executed lines the file yields nothing, not even constants.
        assert!(mutants(&[]).is_empty());
    }

    #[test]
    fn a_mutant_changes_exactly_its_span() {
        let order = generate(Path::new("phy.rs"), SOURCE, &BTreeSet::from([5, 6]))
            .unwrap()
            .into_iter()
            .find(|m| m.operator == Operator::Order)
            .unwrap();
        let changed = order.apply(SOURCE);
        let write = changed.find("bus.a.write").unwrap();
        let modify = changed.find("bus.b.modify").unwrap();
        assert!(modify < write);
        assert_eq!(changed.len(), SOURCE.len());
    }
}
