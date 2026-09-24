// Portal navigation groups owner documents without copying their contracts.
export const isLegacyReference = source => /^tools\/blobray\/docs\/[^/]+\.md$/.test(source);

export function groupReferences(documents, used, definitions) {
  const groups = definitions.map(definition => ({ ...definition, sources: [] }));
  for (const source of documents) {
    if (used.has(source) || isLegacyReference(source)) continue;
    const group = groups.find(candidate => candidate.prefixes.some(prefix => source.startsWith(prefix)));
    if (!group) throw new Error(`No reference owner for ${source}`);
    group.sources.push(source);
  }
  return groups.filter(group => group.sources.length);
}
