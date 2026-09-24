import fs from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { parse } from 'parse5';
import { chromium } from 'playwright';
import { config, files, htmlWalk, output, readPortalHtml, write } from './lib.mjs';
import { serve } from './serve.mjs';

export async function checkLinks(site) {
  const inventory = await files(site);
  const paths = new Set(inventory.map(file => path.relative(site, file)));
  const anchors = new Map();
  let linkCount = 0;
  let size = 0;
  for (const file of inventory) {
    size += (await fs.stat(file)).size;
    if (!file.endsWith('.html')) continue;
    const relative = path.relative(site, file);
    const ids = new Set();
    const document = await readPortalHtml(file, site);
    htmlWalk(parse(document.html + document.container), node => {
      for (const attr of node.attrs ?? []) {
        if (attr.name === 'id' || (node.tagName === 'a' && attr.name === 'name')) ids.add(attr.value);
      }
    });
    anchors.set(relative, ids);
  }
  const errors = [];
  const errorKinds = new Map();
  let errorCount = 0;
  const fail = message => {
    errorCount++;
    if (errors.length < 60) errors.push(message);
    const kind = message.slice(message.indexOf(': ') + 2).replace(/ \([^)]*\)$/, '');
    errorKinds.set(kind, (errorKinds.get(kind) ?? 0) + 1);
  };
  // Keep only anchors across pages. The full API contains millions of links;
  // retaining every source/URL pair would exhaust a hosted runner's heap.
  for (const file of inventory.filter(file => file.endsWith('.html'))) {
    const source = path.relative(site, file);
    const document = await readPortalHtml(file, site);
    htmlWalk(parse(document.html + document.container), node => {
      for (const attr of node.attrs ?? []) {
        if (['href', 'src', 'data-src'].includes(attr.name) && attr.value) {
          linkCount++;
          checkLink(source, attr.value);
        }
      }
    });
  }
  function checkLink(source, link) {
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(link)) return;
    const url = new URL(link, `https://portal.invalid${config.basePath}${source}`);
    if (!url.pathname.startsWith(config.basePath)) {
      fail(`${source}: link outside Pages base: ${link}`); return;
    }
    let target = decodeURIComponent(url.pathname.slice(config.basePath.length));
    if (!target || target.endsWith('/')) target += 'index.html';
    if (!paths.has(target)) fail(`${source}: missing ${link} (${target})`);
    else if (url.hash && anchors.has(target)) {
      const fragment = decodeURIComponent(url.hash.slice(1));
      const range = /^(\d+)-(\d+)$/.exec(fragment);
      const sourceRange = target.includes('/src/') && range && Number(range[1]) <= Number(range[2])
        && anchors.get(target).has(range[1]) && anchors.get(target).has(range[2]);
      if (!anchors.get(target).has(fragment) && !anchors.get(target).has(url.hash.slice(1)) && !sourceRange) fail(`${source}: missing anchor ${link}`);
    }
  }
  if (errorCount) throw new Error(`${errorCount} broken local links:\n${errors.join('\n')}\nBy destination:\n${JSON.stringify(Object.fromEntries(errorKinds), null, 2)}`);
  // Conservative decimal interpretation of GitHub Pages' 1 GB site limit.
  if (size > 1_000_000_000) throw new Error(`Portal is ${size} bytes; exceeds Pages 1 GB limit. No API scope was omitted.`);
  return { files: inventory.length, html: anchors.size, links: linkCount, bytes: size };
}

export async function checkBrowser(site, build) {
  const { server, url } = await serve(site);
  let browser;
  const failures = [];
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    page.on('pageerror', error => failures.push(error.message));
    page.on('response', response => {
      if (response.url().startsWith(url) && response.status() >= 400) failures.push(`${response.status()} ${response.url()}`);
    });
    for (const diagram of build.diagrams) {
      await page.goto(url + diagram.page);
      await page.waitForFunction(count => document.querySelectorAll('.mermaid svg').length === count, diagram.count);
      if (await page.locator('.mermaid .error-icon, .mermaid .error-text').count()) throw new Error(`Mermaid error: ${diagram.page}`);
    }
    await page.goto(url);
    await page.getByRole('link', { name: 'From binary evidence to a Wi-Fi station', exact: true }).first().click();
    await page.waitForURL('**/guide/docs/binary-to-station.html');
    await page.goto(`${url}?search=ScanPhy`);
    await page.locator('#mdbook-searchresults a').first().waitFor();
    await page.locator('#mdbook-searchresults a').first().click();
    await page.waitForLoadState('domcontentloaded');
    if (build.full) {
      for (const purpose of ['public-rustdoc', 'private-rustdoc']) {
        const entry = build.entries.find(e => e.package === 'oer-wifi-sta' && e.purpose === purpose);
        if (!entry) throw new Error(`No ${purpose} station API`);
        await page.goto(`${url}${entry.url}?search=StaCandidateScanService`);
        const result = page.locator('#search a[href*="struct.StaCandidateScanService.html"]').first();
        await result.waitFor();
        await result.click();
        await page.waitForLoadState('domcontentloaded');
        const source = page.locator('a.src').first();
        await source.waitFor();
        await source.click();
        await page.waitForURL('**/src/**');
        await page.locator('[id="1"]').waitFor();
        await page.waitForFunction(() => {
          const id = location.hash.slice(1).split('-')[0];
          const bounds = document.getElementById(id)?.getBoundingClientRect();
          return bounds && bounds.top >= 0 && bounds.top < innerHeight;
        });
      }
      const hidden = build.apiLinks.find(link => link.after.includes('#method.') && link.source.startsWith('api/public-rustdoc/'));
      if (hidden) {
        await page.goto(url + hidden.after);
        await page.waitForFunction(() => {
          const bounds = document.getElementById(decodeURIComponent(location.hash.slice(1)))?.getBoundingClientRect();
          return bounds && bounds.top >= 0 && bounds.top < innerHeight;
        });
      }
      const chip = build.entries.find(e => e.package === 'oer-esp32s31-hal' && e.purpose === 'public-rustdoc');
      if (!chip) throw new Error('No target HAL API');
      await page.goto(url + chip.url);
      await page.locator('main').waitFor();
      await page.locator('meta[name="rustdoc-vars"]').waitFor({ state: 'attached' });
      const failedPage = await browser.newPage();
      await failedPage.route('**/api/content/*.html.gz', route => route.fulfill({ status: 404, body: 'Missing fixture' }));
      await failedPage.goto(url + chip.url);
      await failedPage.getByRole('alert').filter({ hasText: 'HTTP 404' }).waitFor();
      await failedPage.close();
      const unsupportedPage = await browser.newPage();
      await unsupportedPage.addInitScript(() => { delete globalThis.DecompressionStream; });
      await unsupportedPage.goto(url + chip.url);
      await unsupportedPage.getByRole('alert').filter({ hasText: 'gzip decompression support' }).waitFor();
      await unsupportedPage.close();
      const noScriptPage = await browser.newPage({ javaScriptEnabled: false });
      await noScriptPage.goto(url + chip.url);
      // Playwright's text locator deliberately excludes noscript elements.
      if (!(await noScriptPage.locator('body').innerText()).includes('API pages require JavaScript')) {
        throw new Error('Missing no-JavaScript API explanation');
      }
      await noScriptPage.close();
    }
    if (failures.length) throw new Error(`Browser failures:\n${failures.join('\n')}`);
    return { diagrams: build.diagrams.reduce((sum, item) => sum + item.count, 0), search: true, apiSearch: build.full };
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await fs.rm(path.join(output, 'check.json'), { force: true });
  const build = JSON.parse(await fs.readFile(path.join(output, 'build.json')));
  const site = path.join(output, 'site');
  const links = await checkLinks(site);
  console.log('Portal links:', links);
  const browser = await checkBrowser(site, build);
  await write(path.join(output, 'check.json'), JSON.stringify({ source: build.source, full: build.full, links, browser }, null, 2));
  console.log('Portal browser:', browser);
}
