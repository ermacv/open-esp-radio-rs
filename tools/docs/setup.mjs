import fs from 'node:fs/promises';
import path from 'node:path';
import { config, git, root, run } from './lib.mjs';

const install = path.join(root, 'target/docs/tooling');
for (const [name, version] of [['mdbook', config.mdbook], ['mdbook-mermaid', config.mermaid]]) {
  run('cargo', ['install', name, '--version', version, '--locked', '--root', install, '--jobs', '2']);
}
run(path.join(root, 'tools/docs/node_modules/.bin/playwright'), ['install', 'chromium']);
run('rustup', ['component', 'add', 'llvm-tools-preview']);
for (const manifest of git('ls-files', '--cached', '--others', '--exclude-standard', '*Cargo.toml').split('\n').filter(Boolean)) {
  if (/^\[workspace\]\s*$/m.test(await fs.readFile(path.join(root, manifest), 'utf8'))) {
    run('cargo', ['fetch', '--locked', '--manifest-path', manifest]);
  }
}
