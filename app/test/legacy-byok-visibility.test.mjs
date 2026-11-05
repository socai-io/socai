import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';
import { runInNewContext } from 'node:vm';

// Exercise the account renderer with the model catalog returned by the backend.
const source = await readFile(new URL('../src/panels/tasks.ts', import.meta.url), 'utf8');
const start = source.indexOf('  export function renderAccountConfig()');
const end = source.indexOf('  function renderConfigContent()', start);
assert.ok(start >= 0 && end > start);
const renderer = stripTypeScriptTypes(source.slice(start, end).replace('export ', ''));

function render(models, selectedProvider = 'socai', loggedIn = true) {
  const context = {
    authMenu: { isLoggedIn: () => loggedIn },
    modelsCache: models,
    modelProvider: selectedProvider,
    selectedModel: () => models.find(model => model.provider === selectedProvider),
    savingKey: false, switchingKeyMode: false, keyError: '',
    esc: String, t: key => key,
    renderConfigContent: () => '<input id="legacy-key-input" />',
  };
  return runInNewContext(renderer + '\nrenderAccountConfig()', context);
}

const hosted = { provider: 'socai', has_key: true };
const legacy = { provider: 'deepseek', has_key: true };

test('hosted credentials do not expose API-key setup to new or non-legacy users', () => {
  assert.equal(render([hosted]), '');
  assert.equal(render([{ ...hosted, has_key: false }]), '');
  assert.equal(render([]), '');
});

test('legacy users keep the toggle after switching to the hosted model', () => {
  const html = render([hosted, legacy]);
  assert.match(html, /id="agent-own-key"/);
  assert.doesNotMatch(html, /id="legacy-key-input"|\bchecked\b/);
});

test('legacy users retain key editing, including after a credential stops being available', () => {
  for (const has_key of [true, false]) {
    const html = render([hosted, { ...legacy, has_key }], 'deepseek');
    assert.match(html, /\bchecked\b/);
    assert.match(html, /id="legacy-key-input"/);
  }
});

test('signed-out users have no key setup even with a cached legacy catalog', () => {
  assert.equal(render([hosted, legacy], 'deepseek', false), '');
});
