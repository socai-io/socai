// Exercise the native installer only in a new temporary project, never user skills.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtemp, mkdir, readFile, realpath, rename, writeFile} from 'node:fs/promises';
import {join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const binary = resolve(process.argv[2]);
const stage = await realpath(await mkdtemp('/tmp/socai-integration-check-'));
const project = join(stage, 'project');
await mkdir(project);
await mkdir(join(project, '.git'));
const source = fileURLToPath(new URL('../plugins/socai-social-research/skills/socai-social-research/', import.meta.url));
const files = ['SKILL.md', 'references/commands.md', 'references/connection-troubleshooting.md'];
const checks = [];
const invoke = (...args) => spawnSync(binary, ['integrate', ...args, '--scope', 'project', '--json'], {
  cwd: project,
  env: {...process.env, SOCAI_TELEMETRY: 'off'},
  encoding: 'utf8',
  timeout: 15000,
});
try {
  const install = invoke('install', 'all');
  assert.equal(install.status, 0, install.stderr);
  const targets = JSON.parse(install.stdout);
  assert.equal(targets.length, 11);
  const paths = [...new Set(targets.map(target => target.path))];
  assert.equal(paths.length, 10);
  for (const path of paths) {
    assert(path.startsWith(project + '/'), 'Installation escaped the isolated project');
    for (const file of files) {
      const actual = await readFile(join(path, file), 'utf8').catch(error => {
        assert.fail(`Installer omitted ${file}: ${error.code}`);
      });
      assert.equal(actual, await readFile(join(source, file), 'utf8'));
    }
  }
  checks.push('All 11 host entries include all three source-matching files');
  assert(targets.every(target => target.installed && target.up_to_date));
  const target = paths[0];
  const reference = join(target, files[2]);
  const heldReference = join(stage, 'held-reference.md');
  await rename(reference, heldReference);
  const status = invoke('status');
  assert.equal(status.status, 0, status.stderr);
  assert(JSON.parse(status.stdout).filter(row => row.path === target).every(row => row.installed && !row.up_to_date));
  checks.push('Missing reference is reported as outdated');
  const preview = invoke('install', 'codex', '--force', '--dry-run');
  assert.equal(preview.status, 0, preview.stderr);
  assert(JSON.parse(preview.stdout).every(row => !row.up_to_date && row.would_install));
  assert.equal(await readFile(reference, 'utf8').catch(error => error.code), 'ENOENT');
  const rejected = invoke('install', 'codex');
  assert.notEqual(rejected.status, 0);
  assert.equal(await readFile(reference, 'utf8').catch(error => error.code), 'ENOENT');
  assert.equal(await readFile(heldReference, 'utf8'), await readFile(join(source, files[2]), 'utf8'));
  checks.push('Preview and unforced replacement preserve existing files');
  const sentinel = join(project, '.agents', 'preserve-other-settings.txt');
  await writeFile(sentinel, 'leave unrelated settings untouched', {flag: 'wx'});
  const repair = invoke('install', 'codex', '--force');
  assert.equal(repair.status, 0, repair.stderr);
  assert(JSON.parse(repair.stdout).every(row => row.up_to_date));
  assert.equal(await readFile(reference, 'utf8'), await readFile(join(source, files[2]), 'utf8'));
  assert.equal(await readFile(sentinel, 'utf8'), 'leave unrelated settings untouched');
  const repeated = invoke('install', 'all');
  assert.equal(repeated.status, 0, repeated.stderr);
  assert(JSON.parse(repeated.stdout).every(row => row.up_to_date));
  checks.push('Explicit repair restores the reference, preserves unrelated settings, and remains repeatable');
  const receipt = {at: new Date().toISOString(), binary, stage, checks, allVerified: true,
    userSkillsChanged: false, chromeTouched: false};
  await writeFile(join(stage, 'result.json'), JSON.stringify(receipt, null, 2) + '\n', {flag: 'wx'});
  console.log(JSON.stringify(receipt));
} catch (error) {
  console.error(JSON.stringify({stage, checks, allVerified: false, error: error.message}));
  process.exitCode = 1;
}
