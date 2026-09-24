import fs from 'node:fs/promises';
import path from 'node:path';
import { apiEntries, compressApi, config, copySnapshots, files, git, markdown, output, planSnapshots, root, run, sourcePage, transform, verifyReport, write } from './lib.mjs';
import { visit } from 'unist-util-visit';
import { linkApi } from './api-links.mjs';

const full = process.argv.includes('--full');
if (process.argv.slice(2).some(arg => !['--full', '--reuse-checks'].includes(arg))) throw new Error('Usage: node tools/docs/build.mjs [--full] [--reuse-checks]');
if (!process.argv.includes('--reuse-checks')) run('cargo', ['xtask', 'check', 'docs', ...(full ? ['--full', '--export-html'] : [])]);
const scope = `target/docs/${full ? 'gate' : 'static'}`;
const report = JSON.parse(await fs.readFile(path.join(root, scope, 'report.json')));
const plan = JSON.parse(await fs.readFile(path.join(root, scope, 'job-plan.json')));
const revision = git('rev-parse', 'HEAD');
if (report.source.head !== revision || report.scope.mode !== (full ? 'full' : 'static')) {
  throw new Error('Documentation report does not match this build');
}
await verifyReport(plan, report);
await fs.rm(output, { recursive: true, force: true });
const src = path.join(output, 'src');
const site = path.join(output, 'site');
const documents = [...new Set([...plan.documents, 'README.md', ...config.navigation.flatMap(group => group.sources)])]
  .filter(source => !config.sourceOnly.includes(source)).sort();
const pages = new Map(documents.map(source => [source, sourcePage(source)]));
const titles = new Map();
const diagrams = [];
const staged = [];
const sourceStamp = `Source: [${revision.slice(0, 12)}](${config.repository}/tree/${revision})${report.source.dirty ? ' **with local changes**' : ''}.`;
const legacy = source => /^tools\/blobray\/docs\/[^/]+\.md$/.test(source);

async function stage(text, source, destination) {
  const result = await transform(text, source, destination, pages, revision);
  const tree = markdown.parse(result.text);
  let title;
  visit(tree, 'heading', node => {
    if (!title && node.depth === 1) {
      title = '';
      visit(node, child => { if (child.type === 'text' || child.type === 'inlineCode') title += child.value; });
    }
  });
  titles.set(destination, title ?? path.basename(source, '.md'));
  let count = 0;
  visit(tree, 'code', node => { if (node.lang === 'mermaid') count++; });
  if (count) diagrams.push({ page: destination.replace(/\.md$/, '.html'), count });
  await write(path.join(src, destination), result.text);
  for (const asset of result.assets) {
    await fs.mkdir(path.dirname(path.join(src, asset.destination)), { recursive: true });
    await fs.copyFile(asset.source, path.join(src, asset.destination));
  }
  staged.push(destination);
}

for (const source of documents) {
  let text = await fs.readFile(path.join(root, source), 'utf8');
  if (legacy(source)) {
    text = text.replace(/^(# .*)\n/, '$1\n\n> Legacy reference: its command grammar is not supported by current `cargo blobray`.\n');
  }
  text += `\n\n---\n\n${sourceStamp} [View source](${config.repository}/blob/${revision}/${source}) · [Edit original](${config.repository}/edit/main/${source})\n`;
  await stage(text, source, pages.get(source));
}

// Catalog Markdown already contains the evaluator's declarative semantics.
// Render every declared static output; do not synthesize verdicts or read evidence.
const statusPages = [];
for (const directory of report.outputs['static-catalogs']) {
  const chip = path.basename(path.dirname(directory));
  for (const file of (await files(path.join(root, directory))).filter(file => file.endsWith('.md'))) {
    pages.set(path.relative(root, file), `status/${chip}/${path.basename(file)}`);
  }
}
for (const directory of report.outputs['static-catalogs']) {
  const chip = path.basename(path.dirname(directory));
  for (const file of (await files(path.join(root, directory))).filter(file => file.endsWith('.md'))) {
    const destination = `status/${chip}/${path.basename(file)}`;
    const text = `${await fs.readFile(file, 'utf8')}\n\n---\n\n${sourceStamp} Evidence: **not-evaluated** (static declarations only).\n`;
    await stage(text, path.relative(root, file), destination);
    statusPages.push(destination);
  }
}
if (!statusPages.length) throw new Error('No static qualification catalogs were exported');

const entries = full ? await planSnapshots(apiEntries(plan, report)) : [];
const apiIndex = ['# API documentation', '', sourceStamp, '', full
  ? 'Each entry names its target and Cargo feature arguments. Public and private views are separate. Equivalent requirements and byte-identical snapshots share files without dropping configurations. Each snapshot documents one crate; cross-crate implementor lists are limited to the indexes rustdoc emitted.'
  : '**Guide preview:** API snapshots are not included. Run `node tools/docs/build.mjs --full` for the complete public/private matrix.', ''];
if (full) {
  apiIndex.push('Private exports include `#[doc(hidden)]` items. References to those items enter the matching private configuration. API pages require JavaScript and a modern browser with gzip decompression support. Every page retains its original URL and content; guides and status pages are ordinary HTML.', '');
  apiIndex.push('| Package / Cargo target | Visibility | Target | Feature arguments |', '| --- | --- | --- | --- |');
  for (const e of entries) apiIndex.push(`| [${e.package} / ${e['cargo-target']}](${e.url.replace(/^api\//, '')}) | ${e.purpose.replace('-rustdoc', '')} | ${e.target} | ${e.features.length ? e.features.map(f => `\`${f}\``).join(' ') : 'default'} |`);
}
await write(path.join(src, 'api/index.md'), apiIndex.join('\n') + '\n');
const statusIndex = ['# Capability map', '', sourceStamp, '',
  'These generated views describe source declarations and selected program boundaries. Evidence is **not-evaluated**. They do not assert on-air readiness or load vendor/HIL results.', '',
  ...statusPages.map(page => `- [${titles.get(page)}](${page.replace(/^status\//, '')})`), '',
  `Read [qualification](${path.posix.relative('status', pages.get('qualification/README.md'))}) for evaluation with applicable evidence.`, '',
  '## Selected programs', '', ...plan['catalog-groups'].flatMap(group => group.programs.map(program => `- [${path.basename(program, '.toml')}](${config.repository}/blob/${revision}/${program})`)), ''];
await write(path.join(src, 'status/index.md'), statusIndex.join('\n'));

const summary = ['# Summary', '', '[Open ESP Radio](index.md)', ''];
const used = new Set(['README.md']);
for (const group of config.navigation) {
  summary.push(`# ${group.title}`, '');
  for (const source of group.sources) {
    summary.push(`- [${titles.get(pages.get(source))}](${pages.get(source)})`);
    used.add(source);
  }
  summary.push('');
}
summary.push('# API and capability map', '', '- [API documentation](api/index.md)', '- [Capability map](status/index.md)');
for (const page of statusPages) summary.push(`  - [${titles.get(page)}](${page})`);
summary.push('', '# Component references', '');
for (const source of documents.filter(source => !used.has(source) && !legacy(source))) {
  summary.push(`- [${titles.get(pages.get(source))}](${pages.get(source)})`);
}
summary.push('', '# Legacy references', '');
for (const source of documents.filter(legacy)) summary.push(`- [Legacy: ${titles.get(pages.get(source))}](${pages.get(source)})`);
await write(path.join(src, 'SUMMARY.md'), summary.join('\n') + '\n');

const toolBin = path.join(root, 'target/docs/tooling/bin');
const env = { ...process.env, PATH: `${toolBin}${path.delimiter}${process.env.PATH}` };
await write(path.join(output, 'book.toml'), '[book]\ntitle = "Generated Mermaid assets"\n');
run(path.join(toolBin, 'mdbook-mermaid'), ['install', output], { env });
run(path.join(toolBin, 'mdbook'), ['build', root], { env });

// Copy only successful exports, preserving snapshot boundaries and resources.
const snapshotResources = await copySnapshots(entries, site);
const apiLinks = full ? await linkApi(entries, site) : [];
const compression = full ? await compressApi(site) : null;
await write(path.join(site, '.nojekyll'), '');
await write(path.join(output, 'build.json'), JSON.stringify({
  full, source: report.source, diagrams, entries, snapshotResources, apiLinks, compression, documents: documents.length,
  pages: [...staged.map(page => page.replace(/\.md$/, '.html')), 'api/index.html', 'status/index.html'],
}, null, 2));
console.log(`Portal built: ${documents.length} documents, ${entries.length} API requirements; ${site}`);
