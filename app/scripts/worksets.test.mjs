import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {promptFields, nextPromptField} from '../src/lib/prompt-template-model.ts';

test('finds only explicit editable placeholders, using textarea UTF-16 offsets', () => {
  const value = '☕研究【你的研究关键词】，面向【你的目标受众】。保留【原文】和 [1]。';
  const fields = promptFields(value);
  assert.deepEqual(fields.map(f => f.label), ['你的研究关键词', '你的目标受众']);
  for (const field of fields) assert.equal(value.slice(field.start, field.end), `【${field.label}】`);
  assert.equal(promptFields('Research [[Your keyword]] and [[Your audience]]').length, 2);
  assert.deepEqual(promptFields('【原文】、[1]、[[citation]]、【你的跨\n行】'), []);
});

test('Tab advances after typing and Shift+Tab goes back without trapping focus', () => {
  const value = '研究【你的研究关键词】，面向【你的目标受众】。';
  const [first, last] = promptFields(value);
  assert.deepEqual(nextPromptField(value, first.start, first.end), last);
  assert.deepEqual(nextPromptField(value, last.start, last.end, true), first);
  assert.equal(nextPromptField(value, last.start, last.end), undefined);
  assert.equal(nextPromptField(value, first.start, first.end, true), undefined);
  const edited = value.replace('【你的研究关键词】', '咖啡机');
  assert.equal(nextPromptField(edited, 5, 5)?.label, '你的目标受众');
  assert.deepEqual(promptFields(edited.replace('【你的目标受众】', '租房新手')), []);
});

test('worksets replace the nested tree with an accessible animated dialog', async () => {
  const source = await readFile(new URL('../src/lib/task-examples.ts', import.meta.url), 'utf8');
  const css = await readFile(new URL('../src/lib/task-examples.css', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /<details|data-example-group/);
  assert.match(source, /<dialog/);
  assert.match(source, /showModal/);
  assert.match(source, /data-workset-trigger/);
  assert.match(source, /data-example-alternate/);
  assert.match(css, /backdrop-filter:\s*blur/);
  assert.match(css, /prefers-reduced-motion/);
});

test('worksets omit decorative hints and question counts without removing actions', async () => {
  const source = await readFile(new URL('../src/lib/task-examples.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /task-worksets__hint|task-workset-hint|data-workset-count|task-workset__footer/);
  assert.match(source, /aria-labelledby="task-workset-title"/);
  for (const action of ['data-workset-search', 'data-workset-close', 'data-example-alternate', 'data-keep-draft', 'data-replace-draft']) assert.ok(source.includes(action));
});

test('template binding preserves native input, guards drafts and blocks unresolved submission', async () => {
  const source = await readFile(new URL('../src/lib/prompt-template.ts', import.meta.url), 'utf8');
  const examples = await readFile(new URL('../src/lib/task-examples.ts', import.meta.url), 'utf8');
  assert.match(source, /setSelectionRange/);
  assert.match(source, /isComposing/);
  assert.match(source, /stopImmediatePropagation/);
  assert.match(source, /addEventListener\("submit"/);
  assert.match(examples, /confirm/);
  assert.doesNotMatch(examples, /composer-send.*click/);
});

test('background shell redraw captures and restores open workset state after rebinding', async () => {
  const main = await readFile(new URL('../src/main.ts', import.meta.url), 'utf8');
  const examples = await readFile(new URL('../src/lib/task-examples.ts', import.meta.url), 'utf8');
  assert.ok(main.indexOf('const restoreWorkset = preserveTaskWorkset()') < main.indexOf('root.innerHTML ='));
  assert.ok(main.indexOf('restoreWorkset();') > main.indexOf('agentPanel.bind(state)'));
  for (const state of ['state.query', 'state.indices', 'state.pending', 'state.selection', 'state.scroll']) assert.ok(examples.includes(state));
  assert.match(examples, /!dialog.isConnected \|\| dialog.open/);
});

test('all 44 social templates keep both choices, editable fields and source/safety instructions', async () => {
  const {transpileModule, ScriptTarget, ModuleKind} = createRequire(import.meta.url)('typescript');
  const source = await readFile(new URL('../src/lib/task-examples.ts', import.meta.url), 'utf8');
  const js = transpileModule(source.replace(/^import .*;\n/gm, '').replaceAll('export ', ''), {
    compilerOptions: {target: ScriptTarget.ES2022, module: ModuleKind.None},
  }).outputText;
  const catalog = Function('getLanguage', 'platformLabel', 'esc', 'platformIcon', js + '\nreturn {taskExamples};')(
    () => 'zh', p => p === 'xhs' ? '小红书' : p === 'dy' ? '抖音' : p, String, () => '',
  );
  for (const chinese of [true, false]) {
    for (const platform of ['xhs', 'dy']) {
      const examples = catalog.taskExamples(platform, chinese);
      assert.equal(examples.length, 11);
      assert.equal(new Set(examples.map(e => e.id)).size, 11);
      for (const example of examples) {
        assert.equal(example.prompts.length, 2);
        assert.notEqual(example.prompts[0], example.prompts[1]);
        for (const prompt of example.prompts) assert.ok(promptFields(prompt).length, example.id + ': ' + prompt);
      }
    }
  }
  const comments = catalog.taskExamples('xhs', true).find(e => e.id === 'xhs-comment-collection');
  assert.match(comments.prompts[0], /不把已加载当全部/);
  const leads = catalog.taskExamples('dy', true).find(e => e.id === 'dy-comment-leads');
  assert.match(leads.prompts[0], /不要自动评论或私信/);
});
