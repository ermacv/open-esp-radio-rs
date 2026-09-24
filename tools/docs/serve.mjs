import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { config, output } from './lib.mjs';

const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.woff': 'font/woff', '.woff2': 'font/woff2', '.txt': 'text/plain' };
export async function serve(directory = path.join(output, 'site'), port = 0) {
  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://localhost');
      if (!url.pathname.startsWith(config.basePath)) throw new Error('Outside base path');
      const relative = decodeURIComponent(url.pathname.slice(config.basePath.length));
      let file = path.resolve(directory, relative);
      if (!file.startsWith(`${path.resolve(directory)}${path.sep}`) && file !== path.resolve(directory)) throw new Error('Outside site');
      if ((await fs.stat(file)).isDirectory()) file = path.join(file, 'index.html');
      const content = await fs.readFile(file);
      response.writeHead(200, { 'Content-Type': types[path.extname(file)] ?? 'application/octet-stream' });
      response.end(content);
    } catch {
      response.writeHead(404);
      response.end('Not found');
    }
  });
  await new Promise(resolve => server.listen(port, '127.0.0.1', resolve));
  return { server, url: `http://127.0.0.1:${server.address().port}${config.basePath}` };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { url } = await serve(undefined, 4173);
  console.log(`Preview: ${url}`);
}
