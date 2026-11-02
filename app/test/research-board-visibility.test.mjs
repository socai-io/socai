// Actual renderer, no browser, task execution or business acceptance.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
const source=await readFile(new URL('../src/panels/research-board.ts',import.meta.url),'utf8');
const code=stripTypeScriptTypes(source.replace(/^import[^\n]*\n/gm,'').replace(/\bexport /g,''));
const context={getLanguage:()=> 'zh',esc:String,renderLeadCitation:(_url,name)=>name,platformIcon:()=>'',platformLabel:String};
runInNewContext(code+'\nglobalThis.renderBoard=renderResearchBoard;',context);
const task=()=>({task_id:'owned',research_mode:false,status:'completed',events:[],notes:[],sites:['xhs']});
test('ordinary content research shows a real saved board without deep mode',()=>{
 const html=context.renderBoard({...task(),research_board:{summary:'Observed sources',leads:[{id:'one',name:'Observed source',platform:'xhs',signal:'Actual signal',url:'https://example.com',stage:'context'}]}});
 assert.match(html,/data-board-task="owned"/);assert.match(html,/Observed source/);
});
test('a real early plan is visible before leads arrive',()=>{
 assert.match(context.renderBoard({...task(),status:'running',research_board:{summary:'Read actual sources'}}),/Read actual sources/);
});
test('ordinary active or terminal chat without a board does not grow an empty panel',()=>{
 for(const status of ['running','completed'])assert.equal(context.renderBoard({...task(),status}), '');
});
test('deep research retains its existing pending workbench',()=>{
 assert.match(context.renderBoard({...task(),research_mode:true,status:'queued'}),/data-board-task="owned"/);
});
