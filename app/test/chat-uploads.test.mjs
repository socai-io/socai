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
