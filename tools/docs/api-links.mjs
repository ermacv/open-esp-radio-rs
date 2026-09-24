import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { parse } from 'parse5';
import { files, htmlWalk, readPortalHtml, rewritePortalHtml, root, sourcePage } from './lib.mjs';

const escape = value => value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;');
const attr = (node, name) => node.attrs?.find(attribute => attribute.name === name)?.value;
const profile = entry => `${entry.target}/${JSON.stringify(entry.features)}`;

// Link isolated exports only when the target and feature arguments agree.
// Public references to hidden items may enter the matching complete private API.
export async function linkApi(entries, site) {
  const records = new Map();
  for (const entry of entries) {
    const directory = `api/${entry.canonicalSnapshot.split('/rustdoc/')[1]}`;
    if (!records.has(directory)) records.set(directory, { directory, entries: [], items: new Map(), leaves: new Map() });
    records.get(directory).entries.push(entry);
  }
  const inventory = (await files(path.join(site, 'api'))).filter(file => file.endsWith('.html'));
  const paths = new Set(inventory.map(file => path.relative(site, file)));
  const anchors = new Map();
  const digests = new Map();
  const definitions = new Map();
  const redirects = [];
  function owner(file) { return records.get(file.split('/').slice(0, 3).join('/')); }
  function collectAnchors(document) {
    const ids = new Set();
    htmlWalk(document, node => { if (attr(node, 'id')) ids.add(attr(node, 'id')); });
    return ids;
  }
  async function valid(file, fragment = '') {
    if (!paths.has(file)) return false;
    if (!fragment) return true;
    if (!anchors.has(file)) anchors.set(file, collectAnchors(parse((await readPortalHtml(path.join(site, file), site)).html)));
    const ids = anchors.get(file);
    const decoded = decodeURIComponent(fragment);
    const range = /^(\d+)-(\d+)$/.exec(decoded);
    return ids.has(fragment) || ids.has(decoded) || (file.includes('/src/') && range && ids.has(range[1]) && ids.has(range[2]));
  }
  async function definition(file) {
    if (!definitions.has(file)) {
      let identity;
      let foundSource = false;
      htmlWalk(parse((await readPortalHtml(path.join(site, file), site)).html), node => {
        if (foundSource || node.tagName !== 'a' || !attr(node, 'class')?.split(' ').includes('src') || !attr(node, 'href')) return;
        foundSource = true;
        const url = new URL(attr(node, 'href'), `https://api.invalid/${file}`);
        const directory = owner(file).directory;
        if (url.origin === 'https://api.invalid' && url.pathname.startsWith(`/${directory}/src/`)) {
          identity = url.pathname.slice(directory.length + 2) + url.hash;
        }
      });
      definitions.set(file, identity);
    }
    return definitions.get(file);
  }
  for (const file of paths) {
    const record = owner(file);
    if (!record) continue;
    const relative = file.slice(record.directory.length + 1);
    const parts = relative.split('/');
    if (parts[0] !== record.entries[0].crate) continue;
    const leaf = parts.pop();
    const item = /^(?:struct|enum|trait|type|fn|constant|static|union|macro)\.(.+)\.html$/.exec(leaf);
    if (item) {
      if (!record.leaves.has(leaf)) record.leaves.set(leaf, []);
      record.leaves.get(leaf).push(file);
    }
    if (item) parts.push(item[1]);
    else if (leaf !== 'index.html') continue;
    const name = parts.join('::');
    if (!record.items.has(name)) record.items.set(name, []);
    record.items.get(name).push(file);
  }
  async function unique(candidates, source, link) {
    const distinct = [...new Set(candidates)];
    if (distinct.length < 2) return distinct[0];
    for (const file of distinct) {
      const pathname = file.split('#')[0];
      if (!digests.has(pathname)) digests.set(pathname, createHash('sha256').update((await readPortalHtml(path.join(site, pathname), site)).html).digest('hex'));
    }
    if (new Set(distinct.map(file => digests.get(file.split('#')[0]))).size !== 1) {
      throw new Error(`Ambiguous API reference ${source}: ${link}; candidates: ${distinct.join(', ')}`);
    }
    return distinct.sort()[0];
  }
  async function resolve(record, source, link, target, fragment, rustPath) {
    const sourceEntry = record.entries[0];
    const sameProfiles = candidate => candidate.entries.some(a => record.entries.some(b => profile(a) === profile(b)));
    const relative = target?.slice(record.directory.length + 1);
    const crate = rustPath?.split('::')[0] ?? (relative?.startsWith('src/') ? relative.split('/')[1] : relative?.split('/')[0]);
    const purposes = sourceEntry.purpose === 'public-rustdoc' ? ['public-rustdoc', 'private-rustdoc'] : ['private-rustdoc'];
    for (const purpose of purposes) {
      const candidates = [...records.values()].filter(candidate => candidate.entries[0].purpose === purpose
        && candidate.entries[0].crate === crate && sameProfiles(candidate));
      const own = candidates.filter(candidate => candidate.entries.some(a => record.entries.some(b => a['cargo-target'] === b['cargo-target'] && a.manifest === b.manifest)));
      for (const group of [own, candidates.filter(candidate => !own.includes(candidate) && candidate.entries[0]['cargo-target'].startsWith('lib:'))]) {
        const found = [];
        for (const candidate of group) {
          if (rustPath) {
            for (const file of candidate.items.get(rustPath) ?? []) if (await valid(file)) found.push(file);
          } else {
            const file = `${candidate.directory}/${relative}`;
            if (await valid(file, fragment)) found.push(file + (fragment ? `#${fragment}` : ''));
            else if (paths.has(target)) {
              // Including hidden/private items can move an inlined public
              // reexport to its defining module. Match the exact source span,
              // not just a coincidentally equal item name.
              const identity = await definition(target);
              if (identity) for (const canonical of candidate.leaves.get(path.basename(target)) ?? []) {
                if (await definition(canonical) === identity && await valid(canonical, fragment)) {
                  found.push(canonical + (fragment ? `#${fragment}` : ''));
                }
              }
            }
          }
        }
        const result = await unique(found, source, link);
        if (result) return result;
      }
    }
  }
  for (const source of paths) {
    const record = owner(source);
    if (!record) continue;
    const original = await readPortalHtml(path.join(site, source), site);
    let html = original.html;
    const document = parse(html, { sourceCodeLocationInfo: true });
    anchors.set(source, collectAnchors(document));
    const links = [];
    htmlWalk(document, node => { if (node.tagName === 'a' && attr(node, 'href')) links.push(node); });
    const edits = [];
    for (const node of links) {
      const link = attr(node, 'href');
      const rustPath = /^[a-zA-Z_]\w*(?:::\w+)+$/.test(link) ? link.replace(/^crate::/, `${record.entries[0].crate}::`) : undefined;
      if (!rustPath && /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(link)) continue;
      const url = new URL(link, `https://api.invalid/${source}`);
      const target = decodeURIComponent(url.pathname.slice(1));
      const fragment = url.hash.slice(1);
      if (!rustPath && await valid(target, fragment)) continue;
      let destination;
      if (!rustPath && link.endsWith('.md') && path.basename(source) === 'index.html') {
        // Rustdoc renders module Markdown links relative to its HTML directory.
        // Resolve them from the actual Rust module before entering owner guides.
        const entry = record.entries[0];
        const module = path.relative(`${record.directory}/${entry.crate}`, path.dirname(source));
        const stem = path.join(root, path.dirname(entry.manifest), 'src', module);
        const sources = [];
        for (const candidate of [`${stem}.rs`, path.join(stem, 'mod.rs')]) {
          if (await fs.stat(candidate).then(s => s.isFile(), () => false)) sources.push(candidate);
        }
        if (sources.length === 1) {
          const original = path.relative(root, path.resolve(path.dirname(sources[0]), link));
          const page = sourcePage(original).replace(/\.md$/, '.html');
          if (await fs.stat(path.join(site, page)).then(s => s.isFile(), () => false)) destination = page;
        }
      }
      if (!destination && (rustPath || target.startsWith(`${record.directory}/`))) {
        destination = await resolve(record, source, link, target, fragment, rustPath);
      }
      if (!destination || destination === target + (fragment ? `#${fragment}` : '')) continue;
      const relative = path.posix.relative(path.posix.dirname(source), destination.split('#')[0]) + (destination.includes('#') ? `#${destination.split('#')[1]}` : '');
      const location = node.sourceCodeLocation.attrs.href;
      edits.push({ start: location.startOffset, end: location.endOffset, value: `href="${escape(relative)}"` });
      redirects.push({ source, before: link, after: destination });
    }
    for (const edit of edits.sort((a, b) => b.start - a.start)) html = html.slice(0, edit.start) + edit.value + html.slice(edit.end);
    const entry = record.entries[0];
    const visibility = entry.purpose === 'private-rustdoc' ? 'Private API, including hidden items' : 'Public API';
    const index = path.posix.relative(path.posix.dirname(source), 'api/index.html');
    const profiles = [...new Set(record.entries.map(e => `${e.target}; ${e.features.join(' ') || 'default features'}`))];
    const banner = `<details class="docblock"><summary>${visibility} · configuration</summary><p>${profiles.map(escape).join('<br>')}<br><a href="${index}">All API configurations</a></p></details>`;
    if (!html.includes(`${visibility} · configuration</summary>`)) {
      html = html.replace('<div class="width-limiter">', `<div class="width-limiter">${banner}`);
    }
    if (html !== original.html) await rewritePortalHtml(path.join(site, source), html, original.container, site);
  }
  return redirects;
}
