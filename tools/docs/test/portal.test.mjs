import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { apiEntries, compressApi, copySnapshots, files, planSnapshots, readPortalHtml, relocateStatic, repairInheritedLinks, root, sourcePage, transform, write } from '../lib.mjs';
import { checkLinks } from '../check.mjs';
import { serve } from '../serve.mjs';

test('full-size API inventories retain every file across directory boundaries', async context => {
  const entry = (name, directory) => ({ name, isDirectory: () => directory,
    isFile: () => !directory, isSymbolicLink: () => false });
  const pages = Array.from({ length: 150_000 }, (_, i) => entry(`${i}.html`, false));
  context.mock.method(fs, 'readdir', async directory => directory === '/fixture'
    ? [entry('api', true)] : pages);
  const inventory = await files('/fixture');
  assert.equal(inventory.length, pages.length);
  assert.ok(inventory.includes('/fixture/api/149999.html'));
});

test('inherited tracing links retain upstream scope and locked version', () => {
  const link = '<a href="dispatcher#setting-the-default-subscriber">default</a>';
  const inherited = `<details><summary><section id="impl-WithSubscriber-for-T"></section></summary><div>${link}<a href="super::Subscriber">Subscriber</a></div></details>`;
  const fixed = repairInheritedLinks(inherited + link, { tracing: '0.1.44' });
  assert.match(fixed, /href="https:\/\/docs.rs\/tracing\/0.1.44\/tracing\/dispatcher\/index.html#setting-the-default-subscriber"/);
  assert.match(fixed, /href="https:\/\/docs.rs\/tracing\/0.1.44\/tracing\/trait.Subscriber.html"/);
  assert.ok(fixed.endsWith(link));
  assert.throws(() => repairInheritedLinks(inherited), /locked tracing version/);
  for (const [scope, href, versions, expected] of [
    ['ExecutableCommand', './index.html#command-api', { crossterm: '0.29.0' }, 'crossterm/0.29.0/crossterm/index.html#command-api'],
    ['FutureExt', 'futures_core::future::TryFuture', { 'futures-core': '0.3.34' }, 'futures-core/0.3.34/futures_core/future/trait.TryFuture.html'],
  ]) {
    const html = `<details><summary><section id="impl-${scope}-for-T"></section></summary><a href="${href}">Reference</a></details>`;
    assert.ok(repairInheritedLinks(html, versions).includes(`https://docs.rs/${expected}`));
  }
});

test('owner links, references, code and catalog links retain their meaning', async () => {
  const pages = new Map([['docs/README.md', sourcePage('docs/README.md')], ['README.md', 'index.md']]);
  const source = '[Home](../README.md#start-here)\n\n[Docs][d]\n\n[d]: README.md\n\n`[Home](../README.md)`\n\n[Code](../Cargo.toml)\n';
  const result = await transform(source, 'docs/example.md', 'guide/docs/example.md', pages, 'abc123');
  assert.match(result.text, /\.\.\/\.\.\/index.md#start-here/);
  assert.match(result.text, /\[d\]: index.md/);
  assert.match(result.text, /`\[Home\]\(\.\.\/README.md\)`/);
  assert.match(result.text, /blob\/abc123\/Cargo.toml/);
  const catalog = await transform('[Policy](../docs/README.md)', 'target/generated.md', 'status/chip/catalog.md', pages, 'abc123');
  assert.match(catalog.text, /\.\.\/\.\.\/guide\/docs\/index.md/);
  const directory = await transform('[Docs](../docs/)', 'docs/example.md', 'guide/docs/example.md', pages, 'abc123');
  assert.match(directory.text, /\(index.md\)/);
  await assert.rejects(transform('[Missing](absent.md)', 'docs/example.md', 'guide/docs/example.md', pages, 'abc123'), /Missing source/);
  await assert.rejects(transform('[Escape](../../../secret)', 'docs/example.md', 'guide/docs/example.md', pages, 'abc123'), /escapes repository/);
});

test('image and HTML destinations are staged without changing explicit anchors', async () => {
  const result = await transform('![Source](../Cargo.toml)\n\n<a id="keep"></a>\n\n<a href="../README.md#start-here">Home</a>', 'docs/example.md', 'guide/docs/example.md', new Map([['README.md', 'index.md']]), 'abc123');
  assert.equal(result.assets[0].source, path.join(root, 'Cargo.toml'));
  assert.match(result.text, /assets\/Cargo.toml/);
  assert.match(result.text, /id="keep"/);
  assert.match(result.text, /\.\.\/\.\.\/index.md#start-here/);
  const reference = await transform('![Source][image]\n\n[image]: ../Cargo.toml', 'docs/example.md', 'guide/docs/example.md', new Map(), 'abc123');
  assert.equal(reference.assets.length, 1);
  assert.match(reference.text, /assets\/Cargo.toml/);
});

test('API requirements require complete same-visibility exported snapshots', () => {
  const plan = { schema: 2, jobs: [{ purpose: 'public-rustdoc', configuration: { id: 'default', 'cargo-target': 'lib:example' } }] };
  const report = { schema: 2, outputs: { 'html-snapshot-export': true, 'exported-html-snapshots': ['target/docs/gate/rustdoc/public-rustdoc/shared'] }, 'requirement-map': [{ requirement: { purpose: 'public-rustdoc', 'configuration-id': 'default' }, execution: { purpose: 'public-rustdoc', 'configuration-id': 'shared' } }] };
  assert.equal(apiEntries(plan, report)[0].url, 'api/public-rustdoc/shared/example/index.html');
  report.outputs['exported-html-snapshots'] = [];
  assert.throws(() => apiEntries(plan, report), /Missing API snapshot/);
  report['requirement-map'][0].execution.purpose = 'private-rustdoc';
  assert.throws(() => apiEntries(plan, report), /Missing API mapping/);
});

test('HTML checker and preview enforce the Pages subpath, resources and anchors', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'oer-portal-'));
  let server;
  try {
    await write(path.join(directory, 'index.html'), '<a href="nested/page.html#ok">Next</a>');
    await write(path.join(directory, 'nested/page.html'), '<h1 id="ok">Title</h1><script src="../script.js"></script>');
    await write(path.join(directory, 'script.js'), '');
    assert.equal((await checkLinks(directory)).html, 2);
    const preview = await serve(directory);
    server = preview.server;
    assert.equal((await fetch(preview.url + 'nested/page.html')).status, 200);
    assert.equal((await fetch(new URL('/index.html', preview.url))).status, 404);
    await write(path.join(directory, 'index.html'), '<a href="nested/page.html#missing">Bad</a>');
    await assert.rejects(checkLinks(directory), /missing anchor/);
    await fs.rm(path.join(directory, 'script.js'));
    await assert.rejects(checkLinks(directory), /missing .*script.js/);
  } finally {
    if (server) await new Promise(resolve => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test('snapshot deduplication preserves distinct contents and visibility with working shared assets', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'oer-api-'));
  try {
    const sources = ['public-rustdoc/a', 'public-rustdoc/b', 'public-rustdoc/c', 'private-rustdoc/a'];
    const entries = [];
    for (const source of sources) {
      const snapshot = `target/docs/gate/rustdoc/${source}`;
      await write(path.join(directory, snapshot, 'example/index.html'), '<link href="../static.files/style.css" rel="stylesheet"><a href="../src/example/lib.rs.html#1-2">Source</a><script src="../trait.impl/example/trait.Unimplemented.js" async></script><p>' + (source.endsWith('/c') ? 'Different API' : 'Same API') + '</p>');
      await write(path.join(directory, snapshot, 'static.files/style.css'), 'body { color: black; }');
      await write(path.join(directory, snapshot, 'src/example/lib.rs.html'), '<span id="1">One</span><span id="2">Two</span>');
      entries.push({ snapshot, crate: 'example' });
    }
    const planned = await planSnapshots(entries, directory);
    assert.equal(planned[0].url, planned[1].url);
    assert.notEqual(planned[0].url, planned[2].url);
    assert.notEqual(planned[0].url, planned[3].url);
    const site = path.join(directory, 'site');
    const resources = await copySnapshots(planned, site, directory);
    assert.equal(resources.unavailableImplementorIndexes.length, 3);
    assert.match(await fs.readFile(path.join(site, planned[0].url), 'utf8'), /does not enumerate implementations in other crates/);
    assert.equal((await fs.readdir(path.join(site, 'api/static'))).length, 1);
    assert.equal((await checkLinks(site)).html, 9);
    const html = '<script>const font="../static.files/font.woff";</script><meta data-static-root-path="../static.files/"><code>../static.files/style.css</code>';
    const moved = relocateStatic(html, '../static.files/', '../../../static/shared/');
    assert.match(moved, /font="\.\.\/\.\.\/\.\.\/static\/shared\/font.woff"/);
    assert.match(moved, /data-static-root-path="\.\.\/\.\.\/\.\.\/static\/shared\/"/);
    assert.match(moved, /<code>\.\.\/static.files\/style.css<\/code>/);
    const original = await fs.readFile(path.join(site, planned[0].url), 'utf8');
    await write(path.join(site, 'api/index.html'), '<h1>API</h1>');
    const compression = await compressApi(site);
    assert.equal(compression.pages, 9);
    assert.equal((await readPortalHtml(path.join(site, planned[0].url), site)).html, original);
    assert.equal((await checkLinks(site)).html, 10);
    assert.deepEqual(await compressApi(site), compression);
    const payload = (await fs.readdir(path.join(site, 'api/content')))[0];
    await fs.writeFile(path.join(site, 'api/content', payload), 'corrupted gzip');
    await assert.rejects(checkLinks(site));
  } finally {
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test('isolated APIs link matching profiles, hidden members and Rust paths', async () => {
  const { linkApi } = await import('../api-links.mjs');
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'oer-api-links-'));
  const rawDirectory = await fs.mkdtemp(path.join(os.tmpdir(), 'oer-api-raw-'));
  try {
    const entries = [
      { id: 'binary', crate: 'app', 'cargo-target': 'bin:app', purpose: 'public-rustdoc' },
      { id: 'library', crate: 'library', 'cargo-target': 'lib:library', purpose: 'public-rustdoc' },
      { id: 'library', crate: 'library', 'cargo-target': 'lib:library', purpose: 'private-rustdoc' },
    ].map(entry => ({ ...entry, target: 'host', features: [], manifest: 'fixture/Cargo.toml',
      canonicalSnapshot: `target/docs/gate/rustdoc/${entry.purpose}/${entry.id}` }));
    const page = (purpose, id, file) => path.join(directory, 'api', purpose, id, file);
    const html = body => `<html><body><main><div class="width-limiter">${body}</div></main></body></html>`;
    await write(page('public-rustdoc', 'binary', 'app/index.html'), html('<a href="../library/struct.Item.html">Type</a><a href="library::Item">Rust path</a>'));
    await write(page('public-rustdoc', 'library', 'library/struct.Item.html'), html('<a class="src" href="../src/library/lib.rs.html#1-2">Source</a><a href="struct.Item.html#method.hidden">Hidden method</a>'));
    await write(page('private-rustdoc', 'library', 'library/internal/struct.Item.html'), html('<a class="src" href="../../src/library/lib.rs.html#1-2">Source</a><section id="method.hidden">Hidden</section>'));
    for (const purpose of ['public-rustdoc', 'private-rustdoc']) {
      await write(page(purpose, 'library', 'src/library/lib.rs.html'), '<span id="1">One</span><span id="2">Two</span>');
    }
    await write(path.join(directory, 'api/index.html'), '<h1>API configurations</h1>');
    await fs.cp(directory, rawDirectory, { recursive: true });
    const freshLinks = await linkApi(entries, rawDirectory);
    await compressApi(directory);
    const links = await linkApi(entries, directory);
    assert.deepEqual(links, freshLinks);
    for (const file of (await files(rawDirectory)).filter(file => file.endsWith('.html'))) {
      const compressed = path.join(directory, path.relative(rawDirectory, file));
      assert.equal((await readPortalHtml(compressed, directory)).html, await fs.readFile(file, 'utf8'));
    }
    assert.equal(links.length, 3);
    assert.match((await readPortalHtml(page('public-rustdoc', 'library', 'library/struct.Item.html'), directory)).html, /private-rustdoc\/library\/library\/internal\/struct.Item.html#method.hidden/);
    assert.match((await readPortalHtml(page('private-rustdoc', 'library', 'library/internal/struct.Item.html'), directory)).html, /Private API, including hidden items/);
    assert.deepEqual(await linkApi(entries, directory), []);
    await checkLinks(directory);
    const other = { ...entries[1], id: 'other', manifest: 'other/Cargo.toml', canonicalSnapshot: 'target/docs/gate/rustdoc/public-rustdoc/other' };
    await write(page('public-rustdoc', 'other', 'library/struct.Item.html'), html('<p>A different API with the same name</p>'));
    await write(page('public-rustdoc', 'binary', 'app/index.html'), html('<a href="library::Item">Rust path</a>'));
    await assert.rejects(linkApi([...entries, other], directory), /Ambiguous API reference/);
    other.features = ['--all-features'];
    await linkApi([...entries, other], directory);
    assert.doesNotMatch(await fs.readFile(page('public-rustdoc', 'binary', 'app/index.html'), 'utf8'), /other\/library/);
  } finally {
    await fs.rm(directory, { recursive: true, force: true });
    await fs.rm(rawDirectory, { recursive: true, force: true });
  }
});
