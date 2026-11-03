// Actual merge implementation; focused regression, not browser/business acceptance.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
const source = await readFile(new URL('../src/panels/notes.ts', import.meta.url), 'utf8');
const implementation = source.match(/function mergeComments\([^]*?\n\}/)?.[0];
assert.ok(implementation, 'Use the actual notes merge, not a duplicate implementation');
const context = {};
runInNewContext(stripTypeScriptTypes(implementation) + '\nglobalThis.merge=mergeComments;', context);

test('same nickname and text from distinct observed authors remain distinct', () => {
  const rows = context.merge([{author:'same',author_id:'a',text:'same text'}],
    [{author:'same',author_id:'b',text:'same text'}]);
  assert.equal(rows.length, 2);
  assert.deepEqual(Array.from(rows, row=>row.author_id), ['a','b']);
});
test('observed profile URLs disambiguate authors when IDs are unavailable', () => {
  const rows = context.merge([{author:'same',author_url:'https://example.com/a',text:'same text'}],
    [{author:'same',author_url:'https://example.com/b',text:'same text'}]);
  assert.equal(rows.length, 2);
});
test('a stable comment updates without dropping distinct reply authors', () => {
  const rows = context.merge([{comment_id:'parent',text:'old',likes:3,replies:[{author_id:'a',text:'reply'}]}],
    [{comment_id:'parent',text:'new',likes:2,replies:[{author_id:'b',text:'reply'}]}]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].text, 'new');
  assert.equal(rows[0].likes, 3);
  assert.equal(rows[0].replies.length, 2);
});
test('older unidentified records remain readable without invented IDs', () => {
  const rows = context.merge([{author:'old',text:'old text'}],[{author:'old',text:'old text',likes:1}]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].author_id, undefined);
  assert.equal(rows[0].comment_id, undefined);
});
