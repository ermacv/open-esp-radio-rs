import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { config, files, markdown, root } from '../lib.mjs';
import { groupReferences, isLegacyReference } from '../navigation.mjs';

const textOf = node => node.value ?? (node.children ?? []).map(textOf).join('');
const slug = text => text.toLowerCase().replace(/[^\p{L}\p{N}_ -]/gu, '').replaceAll(' ', '-');

test('every original Blobray section bookmark leads to a canonical current reference', async () => {
  // These are the published section anchors before the operator reference split.
  const anchors = JSON.parse(await fs.readFile(new URL('fixtures/blobray-anchors.json', import.meta.url)));
  const index = 'tools/blobray/next/README.md';
  const tree = markdown.parse(await fs.readFile(path.join(root, index), 'utf8'));
  for (const anchor of anchors) {
    const offset = tree.children.findIndex(node => node.type === 'heading' && slug(textOf(node)) === anchor);
    assert.ok(offset >= 0, `Missing existing anchor: ${anchor}`);
    const link = tree.children[offset + 1]?.children?.find(node => node.type === 'link');
    assert.ok(link, `Missing canonical destination: ${anchor}`);
    const [relative, fragment] = link.url.split('#');
    const destination = path.posix.normalize(path.posix.join(path.posix.dirname(index), relative));
    assert.ok(destination.startsWith('tools/blobray/next/reference/'), destination);
    assert.equal(isLegacyReference(destination), false);
    assert.equal(fragment, anchor);
    const target = markdown.parse(await fs.readFile(path.join(root, destination), 'utf8'));
    assert.ok(target.children.some(node => node.type === 'heading' && slug(textOf(node)) === fragment), link.url);
  }
});

test('reference grouping retains current pages once and keeps legacy out of current owners', () => {
  const current = 'tools/blobray/next/reference/ir-traces/README.md';
  const design = 'tools/blobray/docs/design/contracts.md';
  const legacy = 'tools/blobray/docs/tui.md';
  const source = [current, design, legacy, 'crates/radio/README.md', 'docs/README.md'];
  const groups = groupReferences(source, new Set(['docs/README.md']), config.referenceGroups);
  assert.deepEqual(groups.flatMap(group => group.sources).sort(), [current, design, 'crates/radio/README.md'].sort());
  assert.equal(groups.find(group => group.sources.includes(current)).id, 'blobray-operator');
  assert.equal(groups.find(group => group.sources.includes(design)).id, 'blobray-design');
  assert.equal(isLegacyReference(legacy), true);
  assert.throws(() => groupReferences(['unknown/README.md'], new Set(), config.referenceGroups), /No reference owner/);
});

test('entry routes reach the tool choice, hardware walkthrough and host exercise within two content links', async () => {
  // Follow content links, not the portal sidebar: these routes must also work on GitHub.
  const distances = new Map([['README.md', 0]]);
  const queue = ['README.md'];
  while (queue.length) {
    const source = queue.shift();
    const distance = distances.get(source);
    if (distance === 2) continue;
    const tree = markdown.parse(await fs.readFile(path.join(root, source), 'utf8'));
    const walk = node => {
      if (node.type === 'link' && !/^(?:[a-z]+:|#)/i.test(node.url)) {
        const relative = node.url.split('#')[0];
        if (relative.endsWith('.md')) {
          const destination = path.posix.normalize(path.posix.join(path.posix.dirname(source), relative));
          if (!distances.has(destination)) { distances.set(destination, distance + 1); queue.push(destination); }
        }
      }
      for (const child of node.children ?? []) walk(child);
    };
    walk(tree);
  }
  for (const source of ['tools/blobray/README.md', 'docs/channel-walkthrough.md', 'docs/first-contribution.md', 'tools/blobray/next/README.md']) {
    assert.ok(distances.has(source) && distances.get(source) <= 2, source);
  }
  const references = (await files(path.join(root, 'tools/blobray/next/reference')))
    .filter(file => file.endsWith('/README.md')).map(file => path.relative(root, file));
  assert.equal(groupReferences(references, new Set(), config.referenceGroups).length, 1);
});
