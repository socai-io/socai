// Execute the real draft/card/binding module. Native picker and DOM are mocked;
// these are regression checks, not real-user acceptance.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
const source=await readFile(new URL('../src/lib/chat-uploads.ts',import.meta.url),'utf8');
const code=stripTypeScriptTypes(source.replace(/^import[^\n]*\n/gm,'').replace(/\bexport /g,''));
const mixed=[{id:'word',name:'报告.docx',size:3000,sha256:'a'},{id:'pdf',name:'手册.pdf',size:802000,sha256:'b'},{id:'image',name:'image.png',size:988000,sha256:'c'}];
test('native spreadsheet picker and extracted reading copies are connected',async()=>{
 const native=await readFile(new URL('../src-tauri/src/chat_uploads.rs',import.meta.url),'utf8');
 assert.match(native,/\["xlsx", "xls", "csv", "docx", "pdf"/);
 assert.match(native,/"xlsx" \| "xls" => Some\(spreadsheet_text/);
 assert.match(native,/join\("table-text.txt"\)/);
 const files=[{id:'xlsx',name:'销售.xlsx',size:3000},{id:'xls',name:'旧表.xls',size:4000},{id:'csv',name:'销售.csv',size:100}];
 const {api,listeners}=setup(new Map(),files);api.bindUploads('new',()=>{});await listeners.attach();
 assert.deepEqual(Array.from(api.uploadIds('new')),['xlsx','xls','csv']);
 for(const f of files)assert.ok(api.renderUploads(api.uploadDraft('new'),'new').includes(f.name));
});
function setup(storage=new Map(),pick=mixed){
 const listeners={},context={getLanguage:()=> 'zh',esc:s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),artifactFileIcon:()=>'<svg/>',formatArtifactSize:String,
  localStorage:{getItem:k=>storage.get(k),setItem:(k,v)=>storage.set(k,v)},
  document:{getElementById:()=>({addEventListener:(event,fn)=>listeners.attach=fn}),querySelectorAll:()=>[]},
  invoke:async()=>pick};
 runInNewContext(code+'\nglobalThis.api={uploadDraft,uploadIds,renderUploads,clearUploads,bindUploads,uploadsBusy};',context);
 return {api:context.api,listeners,storage,context};
}
test('native multi-pick renders all original names, icons, sizes and download controls',async()=>{
 const {api,listeners}=setup();api.bindUploads('new',()=>{});await listeners.attach();
 assert.deepEqual(Array.from(api.uploadIds('new')),['word','pdf','image']);
 const html=api.renderUploads(api.uploadDraft('new'),'new');
 for(const file of mixed){assert.ok(html.includes(file.name));assert.ok(html.includes(String(file.size)));assert.ok(html.includes(`data-chat-upload-save="${file.id}"`));}
 assert.equal((html.match(/data-chat-upload-remove=/g)||[]).length,3);
});
test('draft survives reload, remains conversation-scoped and clears only after success',async()=>{
 const first=setup();first.api.bindUploads('new',()=>{});await first.listeners.attach();
 const second=setup(first.storage);assert.equal(second.api.uploadIds('new').length,3);assert.equal(second.api.uploadIds('task-other').length,0);
 second.api.clearUploads('new');assert.equal(setup(first.storage).api.uploadIds('new').length,0);
});
test('sent cards escape filenames and have no remove action',()=>{
 const {api}=setup();const html=api.renderUploads([{...mixed[0],name:'<img src=x onerror=alert(1)>.docx'}]);
 assert.ok(html.includes('&lt;img'));assert.ok(!html.includes('<img'));assert.ok(!html.includes('data-chat-upload-remove='));
});
test('picker failure and too many attachments retain the existing draft',async()=>{
 const {api,listeners,context}=setup();api.bindUploads('new',()=>{});await listeners.attach();
 context.invoke=async()=>{throw Error('Unreadable file');};await listeners.attach();assert.equal(api.uploadIds('new').length,3);assert.match(api.renderUploads(api.uploadDraft('new'),'new'),/Unreadable file/);
 context.invoke=async()=>Array.from({length:8},(_,i)=>({...mixed[0],id:String(i)}));await listeners.attach();assert.equal(api.uploadIds('new').length,3);assert.equal(api.uploadsBusy('new'),false);
});

test('compact history refresh cannot erase verified attachment cards',async()=>{
 const source=await readFile(new URL('../src/panels/tasks.ts',import.meta.url),'utf8');
 const start=source.indexOf('  function mergeSnapshot('),end=source.indexOf('  function statusRank(',start);
 assert.ok(start>=0&&end>start);
 const context={statusRank:s=>({queued:0,running:1,completed:2})[s]};
 runInNewContext(stripTypeScriptTypes(source.slice(start,end))+'\nglobalThis.merge=mergeSnapshot;',context);
 const existing={task_id:'task',run_dir:'turn-1',status:'completed',uploads_by_turn:[mixed]};
 for(const run_dir of ['turn-1','turn-2']){
  const merged=context.merge(existing,{task_id:'task',run_dir,status:'completed'});
  assert.equal(merged.uploads_by_turn[0].length,3);
 }
 const hydrated=context.merge(existing,{task_id:'task',run_dir:'turn-1',status:'completed',uploads_by_turn:[[]]});
 assert.equal(hydrated.uploads_by_turn[0].length,0);
});
