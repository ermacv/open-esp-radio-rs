import { spawnSync } from 'node:child_process';
import { root } from './lib.mjs';

for (const args of [
  ['test', '-p', 'oer-wifi-sta', 'scan::tests', '--locked', '--offline'],
  ['test', '-p', 'blobray-next', '--test', 'functions', 'register_discovery_review_conflicts_and_source_free_export_share_scope', '--locked', '--offline'],
]) {
  const result = spawnSync('cargo', args, { cwd: root, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
  process.stdout.write(result.stdout ?? '');
  process.stderr.write(result.stderr ?? '');
  if (result.error) throw result.error;
  if (result.status !== 0 || !/test result: ok\. [1-9][0-9]* passed/.test(result.stdout)) {
    throw new Error(`Tutorial did not execute a passing test: cargo ${args.join(' ')}`);
  }
}
