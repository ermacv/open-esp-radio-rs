import fs from 'node:fs/promises';
import path from 'node:path';
import { git, root, run } from './lib.mjs';

const base = process.argv[2];
if (!base || !/^[a-f0-9]{40}$/.test(base)) throw new Error('Expected a full base commit SHA');
const changed = git('diff', '--name-only', base, 'HEAD').split('\n').filter(Boolean);
const shared = file => /^(Cargo\.(toml|lock)|rust-toolchain\.toml|\.cargo\/|tools\/repo\/src\/(checks\/(docs|common)|cargo|paths)|tools\/docs\/|book\.toml|\.github\/workflows\/docs\.yml)/.test(file);
if (changed.some(shared)) {
  run('cargo', ['xtask', 'check', 'docs', '--full']);
} else {
  const inputs = changed.filter(file => /\.(rs|toml)$/.test(file) || path.basename(file) === 'Cargo.lock');
  if (inputs.length) {
    const log = await fs.open(path.join(root, 'target/docs/portal/api-plan.log'), 'w');
    try { run('cargo', ['xtask', 'check', 'docs', '--full', '--list'], { stdio: ['ignore', log.fd, 'inherit'] }); }
    finally { await log.close(); }
    const plan = JSON.parse(await fs.readFile(path.join(root, 'target/docs/gate/job-plan.json')));
    const packages = [...new Map(plan.jobs.map(job => [job.configuration.manifest, job.configuration])).values()]
      .sort((a, b) => b.manifest.length - a.manifest.length);
    const names = new Set();
    let full = false;
    for (const input of inputs) {
      // Shared metadata and deleted owners cannot safely be assigned to a leaf.
      if (/^(qualification\/(catalog|targets)|registers)\//.test(input)) continue;
      const owner = packages.find(p => input.startsWith(`${path.posix.dirname(p.manifest)}/`));
      if (!owner || path.basename(input) === 'Cargo.lock') { full = true; break; }
      names.add(owner.package);
    }
    if (full) run('cargo', ['xtask', 'check', 'docs', '--full']);
    else if (names.size) run('cargo', ['xtask', 'check', 'docs', '--private', ...[...names].sort().flatMap(name => ['--package', name])]);
  }
}
