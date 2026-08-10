//! Function identities and compact diagnostic representation.

use super::*;

const MAX_DIAGNOSTICS_PER_CATEGORY: usize = 64;
const MAX_DIAGNOSTIC_FRAGMENTS: usize = 64;
const MAX_DIAGNOSTIC_FRAGMENT_CHARS: usize = 512;

fn diagnostic_kind(message: &str) -> &'static str {
    match message {
        message
            if message.starts_with("symbolic-cfg:")
                && message.contains("has unsupported effects") =>
        {
            "aggregate"
        }
        message
            if message.starts_with("call graph exceeds")
                || message.starts_with("symbolic-cfg: symbolic CFG exceeds") =>
        {
            "analysis-budget"
        }
        message if message.starts_with("call/jump instruction") => "call-boundary",
        message if message.starts_with("unmodeled-memory-load") => "memory-load",
        message if message.starts_with("unmodeled-memory-store") => "memory-store",
        message if message.starts_with("unresolved-memory-write") => "memory-store",
        message if message.starts_with("unresolved-memory-read") => "memory-load",
        message
            if message.starts_with("input-dependent control-flow")
                || message.starts_with("unresolved input-dependent control-flow") =>
        {
            "control-flow"
        }
        message if message.starts_with("unresolved-indirect") => "indirect-control-flow",
        message
            if message.starts_with("unresolved call")
                || message.starts_with("unresolved-call-relocation") =>
        {
            "unresolved-call"
        }
        message if message.contains("composed call result is used without a modeled callee") => {
            "call-result-model"
        }
        message if message.starts_with("unsupported-call-shape") => "call-shape",
        message if message.starts_with("reference-only-poll") => "poll-model",
        message if message.starts_with("reference-only-memory-intrinsic") => "memory-intrinsic",
        message
            if message.contains("budget")
                || message.contains("additional diagnostics omitted")
                || message.contains("additional diagnostic fragments omitted") =>
        {
            "analysis-budget"
        }
        _ => "other",
    }
}

fn diagnostic_site(message: &str) -> Option<u32> {
    let start = message.find(" at 0x")? + " at 0x".len();
    let digits = message[start..]
        .chars()
        .take_while(|character| character.is_ascii_hexdigit())
        .collect::<String>();
    (!digits.is_empty())
        .then(|| u32::from_str_radix(&digits, 16).ok())
        .flatten()
}

fn stable_root_id(kind: &str, root_fragment: &str) -> String {
    // FNV-1a is deliberately used instead of DefaultHasher: generated IR must
    // remain stable across Rust versions and processes.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in kind
        .bytes()
        .chain(std::iter::once(0))
        .chain(root_fragment.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("blocker-{hash:016x}")
}

fn bounded_fragment(fragment: &str) -> String {
    let mut chars = fragment.chars();
    let bounded = chars
        .by_ref()
        .take(MAX_DIAGNOSTIC_FRAGMENT_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}… [fragment truncated]")
    } else {
        bounded
    }
}

pub(super) fn identity(member: Option<&str>, symbol: &str) -> String {
    member.map_or_else(|| symbol.to_owned(), |member| format!("{member}:{symbol}"))
}

pub(super) fn compact_diagnostic(message: &str) -> LinkedDiagnostic {
    let mut fragment_indices = BTreeMap::<String, usize>::new();
    let mut fragments = Vec::<LinkedDiagnosticFragment>::new();
    let mut original_fragments = 0;

    for (ordinal, fragment) in message.split("; ").enumerate() {
        original_fragments += 1;
        if ordinal >= MAX_DIAGNOSTIC_FRAGMENTS {
            continue;
        }
        let fragment = bounded_fragment(fragment);
        if let Some(index) = fragment_indices.get(&fragment).copied() {
            fragments[index].occurrences += 1;
        } else {
            fragment_indices.insert(fragment.clone(), fragments.len());
            fragments.push(LinkedDiagnosticFragment {
                first_ordinal: ordinal,
                occurrences: 1,
                message: fragment,
            });
        }
    }
    if original_fragments > MAX_DIAGNOSTIC_FRAGMENTS {
        fragments.push(LinkedDiagnosticFragment {
            first_ordinal: MAX_DIAGNOSTIC_FRAGMENTS,
            occurrences: original_fragments - MAX_DIAGNOSTIC_FRAGMENTS,
            message: format!(
                "{} additional diagnostic fragments omitted",
                original_fragments - MAX_DIAGNOSTIC_FRAGMENTS
            ),
        });
    }

    let rendered = fragments
        .iter()
        .map(|fragment| {
            if fragment.occurrences == 1 {
                fragment.message.clone()
            } else {
                format!(
                    "{} [repeated {} times]",
                    fragment.message, fragment.occurrences
                )
            }
        })
        .collect::<Vec<_>>()
        .join("; ");

    let root_fragment = fragments
        .first()
        .map_or(message, |fragment| fragment.message.as_str());
    let kind = diagnostic_kind(root_fragment);
    LinkedDiagnostic {
        root_id: stable_root_id(kind, root_fragment),
        kind,
        site: diagnostic_site(root_fragment),
        rendered,
        original_fragments,
        fragments,
    }
}

pub(super) fn compact_diagnostics(messages: &[String]) -> Vec<LinkedDiagnostic> {
    let mut diagnostics = messages
        .iter()
        .take(MAX_DIAGNOSTICS_PER_CATEGORY)
        .map(|message| compact_diagnostic(message))
        .collect::<Vec<_>>();
    if messages.len() > MAX_DIAGNOSTICS_PER_CATEGORY {
        diagnostics.push(compact_diagnostic(&format!(
            "{} additional diagnostics omitted by the linked-IR presentation budget",
            messages.len() - MAX_DIAGNOSTICS_PER_CATEGORY
        )));
    }
    diagnostics
}

pub(super) type SymbolKey = (Option<String>, String, u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_symbolic_cfg_wrapper_as_aggregate() {
        let diagnostic = compact_diagnostic(
            "symbolic-cfg: path to branch 0x1001b5a2 has unsupported effects: \
             unresolved-call-relocation at 0x1001b576: wifi_assert",
        );

        assert_eq!(diagnostic.kind, "aggregate");
    }
}

pub(super) fn symbol_key(symbol: &artifact::ArtifactSymbolDefinition) -> SymbolKey {
    (symbol.member.clone(), symbol.name.clone(), symbol.address)
}

pub(super) struct IrIdentityCatalog {
    symbols: BTreeMap<SymbolKey, String>,
    targets: BTreeMap<u32, String>,
    selectable_symbols: BTreeMap<String, artifact::ArtifactSymbolDefinition>,
}

impl IrIdentityCatalog {
    pub(super) fn new(resolver: &ReferenceResolver, namespace: Option<&str>) -> Self {
        let mut definitions = resolver.symbols.clone();
        definitions.extend(resolver.symbols_by_address.values().cloned());
        definitions.sort_by_key(symbol_key);
        definitions.dedup_by_key(|symbol| symbol_key(symbol));

        let mut base_counts = BTreeMap::<(Option<String>, String), usize>::new();
        for symbol in &definitions {
            *base_counts
                .entry((symbol.member.clone(), symbol.name.clone()))
                .or_default() += 1;
        }
        let symbols = definitions
            .iter()
            .map(|symbol| {
                let base = identity(symbol.member.as_deref(), &symbol.name);
                let duplicate = base_counts
                    .get(&(symbol.member.clone(), symbol.name.clone()))
                    .copied()
                    .unwrap_or_default()
                    > 1;
                let value = if duplicate {
                    format!("{base}@{:#010x}", symbol.address as u32)
                } else {
                    base
                };
                let value = namespace.map_or(value.clone(), |source| format!("{source}::{value}"));
                (symbol_key(symbol), value)
            })
            .collect::<BTreeMap<_, _>>();
        let targets = resolver
            .symbols_by_address
            .iter()
            .map(|(target, symbol)| {
                (
                    *target,
                    symbols
                        .get(&symbol_key(symbol))
                        .expect("target symbol is present in IR identity catalog")
                        .clone(),
                )
            })
            .collect();
        let selectable_symbols = resolver
            .symbols
            .iter()
            .map(|symbol| {
                (
                    symbols
                        .get(&symbol_key(symbol))
                        .expect("primary symbol is present in IR identity catalog")
                        .clone(),
                    symbol.clone(),
                )
            })
            .collect();
        Self {
            symbols,
            targets,
            selectable_symbols,
        }
    }

    pub(super) fn symbol(&self, symbol: &artifact::ArtifactSymbolDefinition) -> String {
        self.symbols
            .get(&symbol_key(symbol))
            .expect("IR symbol is present in identity catalog")
            .clone()
    }

    pub(super) fn target(&self, target: u32) -> String {
        self.targets
            .get(&target)
            .cloned()
            .unwrap_or_else(|| format!("sub_{target:08x}"))
    }

    pub(super) fn selectable_symbol(
        &self,
        identity: &str,
    ) -> Option<&artifact::ArtifactSymbolDefinition> {
        self.selectable_symbols.get(identity)
    }
}
