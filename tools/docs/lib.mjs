import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { gzipSync, gunzipSync } from 'node:zlib';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkStringify from 'remark-stringify';
import { visit } from 'unist-util-visit';
import { parseFragment, serialize } from 'parse5';

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
export const output = path.join(root, 'target/docs/portal');
export const config = JSON.parse(await fs.readFile(path.join(root, 'tools/docs/portal.json')));
export const markdown = unified().use(remarkParse).use(remarkGfm).use(remarkStringify);
export const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
export function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit', ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (${result.status})`);
}
export async function write(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, value);
}
export async function files(dir) {
  const result = [];
  for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
    const file = path.join(dir, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`Unexpected symlink: ${file}`);
    if (entry.isDirectory()) {
      // Full rustdoc trees can exceed JavaScript's argument-count limit.
      for (const child of await files(file)) result.push(child);
    }
    else if (entry.isFile()) result.push(file);
  }
  return result.sort();
}
export function sourcePage(source) {
  if (source === 'README.md') return 'index.md';
  return `guide/${source.replace(/(^|\/)README\.md$/, '$1index.md')}`;
}
export function repoPath(base, relative) {
  const normalized = path.posix.normalize(path.posix.join(base, relative));
  if (normalized === '..' || normalized.startsWith('../') || path.posix.isAbsolute(normalized)) {
    throw new Error(`Link escapes repository: ${base} / ${relative}`);
  }
  return normalized;
}
export function htmlWalk(node, callback) {
  callback(node);
  for (const child of node.childNodes ?? []) htmlWalk(child, callback);
  if (node.content) htmlWalk(node.content, callback);
}

// Rewrite destinations, never code examples. Both source Markdown and generated
// catalogs resolve links relative to the file supplied by their owner.
export async function transform(text, source, destination, pages, revision) {
  const tree = markdown.parse(text);
  const assets = [];
  const links = [];
  const imageReferences = new Set();
  visit(tree, 'imageReference', node => imageReferences.add(node.identifier));
  const resolve = async (url, image = false) => {
    if (!url || /^(?:[a-z][a-z0-9+.-]*:|\/\/|#)/i.test(url)) return url;
    const match = /^([^?#]*)(\?[^#]*)?(#.*)?$/.exec(url);
    const pathname = decodeURIComponent(match[1]);
    const suffix = (match[2] ?? '') + (match[3] ?? '');
    if (pathname.startsWith('/')) throw new Error(`Root-absolute source link in ${source}: ${url}`);
    let target = repoPath(path.posix.dirname(source), pathname);
    const absolute = path.join(root, target);
    const stat = await fs.stat(absolute).catch(() => null);
    if (!stat) throw new Error(`Missing source link in ${source}: ${url}`);
    if (stat.isDirectory()) {
      const readme = path.posix.join(target, 'README.md');
      if (pages.has(readme)) target = readme;
    }
    if (image) {
      if (!stat.isFile()) throw new Error(`Image is not a file: ${target}`);
      const staged = `assets/${target}`;
      assets.push({ source: absolute, destination: staged });
      return path.posix.relative(path.posix.dirname(destination), staged) + suffix;
    }
    if (pages.has(target)) {
      return path.posix.relative(path.posix.dirname(destination), pages.get(target)) + suffix;
    }
    return `${config.repository}/${stat.isDirectory() ? 'tree' : 'blob'}/${revision}/${target.split('/').map(encodeURIComponent).join('/')}${suffix}`;
  };
  visit(tree, node => {
    if (['link', 'image', 'definition'].includes(node.type)) links.push(node);
  });
  for (const node of links) {
    node.url = await resolve(node.url, node.type === 'image' || (node.type === 'definition' && imageReferences.has(node.identifier)));
  }
  // Raw HTML links/images also occur in component documentation.
  const raw = [];
  visit(tree, 'html', node => raw.push(node));
  for (const node of raw) {
    const fragment = parseFragment(node.value);
    const attrs = [];
    htmlWalk(fragment, el => {
      for (const attr of el.attrs ?? []) {
        if (attr.name === 'href' || attr.name === 'src') attrs.push([el, attr]);
      }
    });
    for (const [el, attr] of attrs) attr.value = await resolve(attr.value, el.tagName === 'img');
    // Preserve standalone opening/closing tags and explicit anchors unchanged
    // unless they actually contain a link destination.
    if (attrs.length) node.value = serialize(fragment);
  }
  return { text: markdown.stringify(tree), assets };
}

export function apiEntries(plan, report) {
  if (plan.schema !== 2 || report.schema !== 2 || !report.outputs?.['html-snapshot-export']) {
    throw new Error('Expected successful schema-2 docs report with HTML export');
  }
  const snapshots = new Set(report.outputs['exported-html-snapshots']);
  const mappings = new Map(report['requirement-map'].map(m => [
    `${m.requirement.purpose}/${m.requirement['configuration-id']}`, m.execution,
  ]));
  return plan.jobs.filter(j => ['public-rustdoc', 'private-rustdoc'].includes(j.purpose)).map(job => {
    const c = job.configuration;
    const mapping = mappings.get(`${job.purpose}/${c.id}`);
    if (!mapping || mapping.purpose !== job.purpose) throw new Error(`Missing API mapping: ${c.id}`);
    const snapshot = snapshots.values().find(p => p.endsWith(`/rustdoc/${mapping.purpose}/${mapping['configuration-id']}`));
    if (!snapshot) throw new Error(`Missing API snapshot: ${c.id}`);
    const crate = c['cargo-target'].split(':')[1]?.replaceAll('-', '_');
    if (!crate) throw new Error(`Unknown Cargo target: ${c['cargo-target']}`);
    return { ...c, purpose: job.purpose, snapshot, crate,
      url: `api/${job.purpose}/${mapping['configuration-id']}/${crate}/index.html` };
  });
}

// Match the existing docs gate's source identity before explicitly reusing a
// successful report. A same-HEAD check alone would accept dirty/stale sources.
export async function verifyReport(plan, report) {
  const owned = new Set(plan.documents);
  const candidates = git('ls-files', '--cached', '--others', '--exclude-standard', '-z').split('\0').filter(Boolean);
  const hash = createHash('sha256');
  // Rust PathBuf ordering compares path components, not whole path strings.
  const comparePaths = (a, b) => {
    const left = a.split('/'); const right = b.split('/');
    for (let i = 0; i < Math.min(left.length, right.length); i++) {
      const order = Buffer.compare(Buffer.from(left[i]), Buffer.from(right[i]));
      if (order) return order;
    }
    return left.length - right.length;
  };
  for (const file of [...new Set(candidates)].sort(comparePaths)) {
    if (file.split('/').some(part => ['_oracles', 'target'].includes(part))) continue;
    if (!(owned.has(file) || /\.(rs|toml)$/.test(file) || path.basename(file) === 'Cargo.lock')) continue;
    const stat = await fs.stat(path.join(root, file)).catch(() => null);
    if (!stat?.isFile()) continue;
    hash.update(file); hash.update('\0'); hash.update(await fs.readFile(path.join(root, file))); hash.update('\0');
  }
  if (git('rev-parse', 'HEAD') !== report.source.head || hash.digest('hex') !== report.source.sha256) {
    throw new Error('Docs report is stale for the current source inputs; rerun the checks');
  }
}

async function treeDigest(directory) {
  const hash = createHash('sha256');
  for (const file of await files(directory)) {
    hash.update(path.relative(directory, file)); hash.update('\0');
    hash.update(await fs.readFile(file)); hash.update('\0');
  }
  return hash.digest('hex');
}

export async function planSnapshots(entries, sourceRoot = root) {
  const canonical = new Map();
  const aliases = new Map();
  for (const snapshot of [...new Set(entries.map(entry => entry.snapshot))].sort()) {
    const purpose = snapshot.split('/rustdoc/')[1].split('/')[0];
    const key = `${purpose}:${await treeDigest(path.join(sourceRoot, snapshot))}`;
    if (!canonical.has(key)) canonical.set(key, snapshot);
    aliases.set(snapshot, canonical.get(key));
  }
  return entries.map(entry => {
    const canonicalSnapshot = aliases.get(entry.snapshot);
    return { ...entry, canonicalSnapshot,
      url: `api/${canonicalSnapshot.split('/rustdoc/')[1]}/${entry.crate}/index.html` };
  });
}

// Update generated resource attributes and rustdoc's inline font preloader.
// Source code and prose containing the same path are not rewritten.
export function relocateStatic(html, before, after) {
  const edits = [];
  const prefixes = before.startsWith('../') ? [before] : [`./${before}`, before];
  htmlWalk(parseFragment(html, { sourceCodeLocationInfo: true }), node => {
    for (const attr of node.attrs ?? []) {
      const prefix = prefixes.find(prefix => attr.value.startsWith(prefix));
      if (!['href', 'src', 'data-static-root-path'].includes(attr.name) || !prefix) continue;
      const location = node.sourceCodeLocation?.attrs?.[attr.name];
      if (location) edits.push({ start: location.startOffset, end: location.endOffset,
        value: `${attr.name}="${(after + attr.value.slice(prefix.length)).replaceAll('&', '&amp;').replaceAll('"', '&quot;')}"` });
    }
    if (node.tagName === 'script' && !node.attrs?.some(attr => attr.name === 'src')) {
      for (const child of node.childNodes ?? []) {
        if (child.nodeName === '#text' && child.value.includes(before)) {
          const location = child.sourceCodeLocation;
          let value = child.value;
          for (const prefix of prefixes) value = value.replaceAll(prefix, after);
          edits.push({ start: location.startOffset, end: location.endOffset, value });
        }
      }
    }
  });
  for (const edit of edits.sort((a, b) => b.start - a.start)) html = html.slice(0, edit.start) + edit.value + html.slice(edit.end);
  return html;
}

// rustdoc inherits dependency blanket-implementation prose without resolving
// these links in the upstream crate's scope. Limit the correction to those
// implementation sections and retain the version selected by their workspace.
export function repairInheritedLinks(html, versions) {
  const tracing = new Map([
    ['dispatcher#setting-the-default-subscriber', 'dispatcher/index.html#setting-the-default-subscriber'],
    ['super::Subscriber', 'trait.Subscriber.html'],
    ['super::Span::current()', 'struct.Span.html#method.current'],
    ['crate::Span', 'struct.Span.html'],
  ]);
  const crossterm = new Map([
    ['./trait.Command.html', 'trait.Command.html'],
    ['./trait.ExecutableCommand.html', 'trait.ExecutableCommand.html'],
    ['./trait.QueueableCommand.html', 'trait.QueueableCommand.html'],
    ['./index.html#command-api', 'index.html#command-api'],
  ]);
  const futures = new Map([
    ['futures_core::future::TryFuture', 'future/trait.TryFuture.html'],
    ['TryFuture::Error', 'future/trait.TryFuture.html#associatedtype.Error'],
    ['TryFuture::Ok', 'future/trait.TryFuture.html#associatedtype.Ok'],
  ]);
  const groups = [
    { crate: 'tracing', module: 'tracing', links: tracing, scope: /^impl-(?:Instrument|WithSubscriber)-for-/ },
    { crate: 'crossterm', module: 'crossterm', links: crossterm, scope: /^impl-(?:ExecutableCommand|QueueableCommand)-for-/ },
    { crate: 'futures-core', module: 'futures_core', links: futures, scope: /^impl-(?:FutureExt|TryFutureExt)-for-/ },
  ];
  if (!groups.some(group => [...group.links.keys()].some(link => html.includes(`href="${link}"`)))) return html;
  const edits = [];
  htmlWalk(parseFragment(html, { sourceCodeLocationInfo: true }), node => {
    const href = node.tagName === 'a' && node.attrs.find(attr => attr.name === 'href');
    if (!href) return;
    const group = groups.find(group => group.links.has(href.value));
    if (!group) return;
    let parent = node.parentNode;
    let inherited = false;
    while (parent && !inherited) {
      if (parent.tagName === 'details') {
        const summary = parent.childNodes.find(child => child.tagName === 'summary');
        const section = summary?.childNodes.find(child => child.tagName === 'section');
        const id = section?.attrs.find(attr => attr.name === 'id')?.value ?? '';
        inherited = group.scope.test(id);
      }
      parent = parent.parentNode;
    }
    if (!inherited) return;
    const version = versions?.[group.crate];
    if (!version) throw new Error(`Cannot identify locked ${group.crate} version for inherited documentation`);
    const location = node.sourceCodeLocation.attrs.href;
    edits.push({ start: location.startOffset, end: location.endOffset,
      value: `href="https://docs.rs/${group.crate}/${version}/${group.module}/${group.links.get(href.value)}"` });
  });
  for (const edit of edits.sort((a, b) => b.start - a.start)) html = html.slice(0, edit.start) + edit.value + html.slice(edit.end);
  return html;
}

export async function copySnapshots(entries, site, sourceRoot = root) {
  const staticGroups = new Set();
  const unavailableImplementorIndexes = [];
  const dependencyVersions = new Map();
  for (const snapshot of new Set(entries.map(entry => entry.canonicalSnapshot))) {
    const entry = entries.find(entry => entry.canonicalSnapshot === snapshot);
    if (entry.workspace && !dependencyVersions.has(entry.workspace)) {
      const lock = await fs.readFile(path.join(sourceRoot, path.dirname(entry.workspace), 'Cargo.lock'), 'utf8');
      const versions = {};
      for (const name of ['tracing', 'crossterm', 'futures-core']) {
        const matches = lock.split('[[package]]').filter(block => new RegExp(`^name = "${name}"$`, 'm').test(block))
          .map(block => /^version = "([^"]+)"$/m.exec(block)?.[1]);
        if (matches.length === 1) versions[name] = matches[0];
      }
      dependencyVersions.set(entry.workspace, versions);
    }
    const source = path.join(sourceRoot, snapshot);
    const destination = path.join(site, 'api', snapshot.split('/rustdoc/')[1]);
    const resources = path.join(source, 'static.files');
    const shared = path.join(site, 'api/static', await treeDigest(resources));
    if (!staticGroups.has(shared)) {
      await fs.cp(resources, shared, { recursive: true });
      staticGroups.add(shared);
    }
    const snapshotFiles = await files(source);
    const sourceFiles = new Set(snapshotFiles.map(file => path.relative(source, file)));
    for (const file of snapshotFiles) {
      const relative = path.relative(source, file);
      if (relative.startsWith('static.files/')) continue;
      const target = path.join(destination, relative);
      if (file.endsWith('.html')) {
        const before = path.posix.relative(path.posix.dirname(relative), 'static.files') + '/';
        const after = path.relative(path.dirname(target), shared) + '/';
        let html = relocateStatic(await fs.readFile(file, 'utf8'), before, after);
        html = repairInheritedLinks(html, dependencyVersions.get(entry.workspace));
        const absent = [];
        htmlWalk(parseFragment(html, { sourceCodeLocationInfo: true }), node => {
          if (node.tagName !== 'script' || !node.attrs.some(attr => attr.name === 'async')) return;
          const src = node.attrs.find(attr => attr.name === 'src')?.value;
          if (!src || /^(?:[a-z]+:|\/)/i.test(src)) return;
          const resource = path.posix.normalize(path.posix.join(path.posix.dirname(relative), src));
          // rustdoc emits optional async implementor requests even when it did
          // not write an index (no external crate documentation in --no-deps).
          // Explain this scope on the page instead of serving a broken request.
          if (resource.startsWith('trait.impl/') && !sourceFiles.has(resource)) {
            absent.push(node.sourceCodeLocation);
            unavailableImplementorIndexes.push({ page: path.relative(site, target), index: resource });
          }
        });
        for (const location of absent.sort((a, b) => b.startOffset - a.startOffset)) {
          html = html.slice(0, location.startOffset)
            + '<p class="docblock">No external implementor index was emitted for this isolated crate snapshot. This list does not enumerate implementations in other crates.</p>'
            + html.slice(location.endOffset);
        }
        await write(target, html);
      } else {
        await fs.mkdir(path.dirname(target), { recursive: true });
        await fs.copyFile(file, target);
      }
    }
    const crate = entries.find(entry => entry.canonicalSnapshot === snapshot).crate;
    await write(path.join(destination, 'index.html'), `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=${crate}/index.html"><title>${crate} API</title></head><body><a href="${crate}/index.html">${crate} API</a></body></html>`);
  }
  return { unavailableImplementorIndexes };
}

export async function compressApi(site) {
  const api = path.join(site, 'api');
  const loader = path.join(api, 'load-page.js');
  await fs.copyFile(path.join(root, 'tools/docs/api-loader.js'), loader);
  const payloads = new Set();
  let expandedBytes = 0;
  let compressedBytes = 0;
  let pages = 0;
  for (const file of (await files(api)).filter(file => file.endsWith('.html') && file !== path.join(api, 'index.html'))) {
    const html = Buffer.from((await readPortalHtml(file, site)).html);
    expandedBytes += html.length;
    const digest = createHash('sha256').update(html).digest('hex');
    const payload = path.join(api, 'content', `${digest}.html.gz`);
    if (!payloads.has(digest)) {
      const compressed = gzipSync(html, { level: 9 });
      await write(payload, compressed);
      compressedBytes += compressed.length;
      payloads.add(digest);
    }
    const relative = path.relative(path.dirname(file), payload);
    const script = path.relative(path.dirname(file), loader);
    const home = path.relative(path.dirname(file), path.join(api, 'index.html'));
    await write(file, `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="oer-api-content" content="${relative}" data-sha256="${digest}"><title>API documentation — Open ESP Radio</title><script defer src="${script}"></script></head><body><main><p id="api-loading" aria-live="polite">Loading API documentation…</p><noscript><p>API pages require JavaScript and a modern browser. Guides and the capability map are available without this viewer.</p></noscript><p><a href="${relative}">Download this page (gzip)</a> · <a href="${home}">API index</a></p></main></body></html>`);
    pages++;
  }
  for (const file of await fs.readdir(path.join(api, 'content'))) {
    if (!payloads.has(file.replace(/\.html\.gz$/, ''))) await fs.rm(path.join(api, 'content', file));
  }
  return { pages, uniquePayloads: payloads.size, expandedBytes, compressedBytes };
}

export async function rewritePortalHtml(file, html, container, site) {
  if (!container) return write(file, html);
  const old = /data-sha256="([a-f0-9]{64})"/.exec(container)?.[1];
  if (!old) throw new Error(`Missing API payload identity: ${file}`);
  const digest = createHash('sha256').update(html).digest('hex');
  await write(path.join(site, 'api/content', `${digest}.html.gz`), gzipSync(html, { level: 9 }));
  await write(file, container.replaceAll(old, digest));
}

export async function readPortalHtml(file, site) {
  const container = await fs.readFile(file, 'utf8');
  if (!container.includes('name="oer-api-content"')) return { html: container, container: '' };
  let payload;
  let digest;
  htmlWalk(parseFragment(container), node => {
    if (node.tagName !== 'meta' || !node.attrs.some(attr => attr.name === 'name' && attr.value === 'oer-api-content')) return;
    payload = node.attrs.find(attr => attr.name === 'content')?.value;
    digest = node.attrs.find(attr => attr.name === 'data-sha256')?.value;
  });
  if (!payload) return { html: container, container: '' };
  const target = path.resolve(path.dirname(file), payload);
  if (!target.startsWith(`${path.resolve(site)}${path.sep}`)) throw new Error(`Compressed page escapes site: ${file}`);
  const html = gunzipSync(await fs.readFile(target));
  if (createHash('sha256').update(html).digest('hex') !== digest) throw new Error(`Compressed page identity mismatch: ${file}`);
  return { html: html.toString('utf8'), container };
}
