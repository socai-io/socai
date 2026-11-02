// Execute actual frontend loader code with deferred transport responses.
// This is a focused race regression, not product/business acceptance.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
for(const [name,path] of [["Desktop",new URL("../src/panels/tasks.ts",import.meta.url)]]){
 const source=await readFile(path,'utf8');
 const start=source.indexOf("  // Fetch a task's note archive");
 const end=source.indexOf('  export function loadSelectedTaskNotes',start);
 assert.ok(start>=0&&end>start);
 const code=stripTypeScriptTypes(source.slice(start,end)).replaceAll('export ','');
 function harness(){
  const task={task_id:'owned',run_dir:'run-old',notes:[{note_id:'old'}],readingStats:[]};
  const reads=[];let renders=0;
  const c={tasks:[task],selectedTaskId:'owned',selectedTask:()=>task,
   console:{error(){}},setTaskNotes:(_id,notes)=>{task.notes=notes;return true;},
   invoke:command=>new Promise((resolve,reject)=>reads.push({command,resolve,reject})),
  };
  runInNewContext(code+'\nglobalThis.load=loadTaskNotes;globalThis.read=readTaskNotes;',c);
  return{task,reads,load:()=>c.load('owned',{rerender(){renders++;}}),read:()=>c.read('owned',task.run_dir),renders:()=>renders};
 }
 const resolve=(h,notes)=>h.reads.forEach(r=>r.resolve(r.command==='agent_task_notes'?notes:[]));
 test(name+' concurrent archive loads share a single pair of reads',async()=>{
  const h=harness(),a=h.load(),b=h.load();
  assert.equal(h.reads.length,2);resolve(h,[{note_id:'new'}]);await Promise.all([a,b]);
  assert.equal(h.renders(),1);
 });
 test(name+' late archive response cannot replace a newer run',async()=>{
  const h=harness(),a=h.load();h.task.run_dir='run-new';h.task.notes=[{note_id:'new-run'}];
  resolve(h,[{note_id:'stale'}]);await a;assert.equal(h.task.notes[0].note_id,'new-run');assert.equal(h.renders(),0);
 });
 test(name+' unchanged archives do not trigger a repaint',async()=>{
  const h=harness(),a=h.load();resolve(h,[{note_id:'old'}]);await a;assert.equal(h.renders(),0);
 });
 test(name+' failed archive reads release their flight for retry',async()=>{
  const h=harness(),a=h.load();h.reads.forEach(r=>r.reject(Error('offline')));await a;
  const b=h.load();assert.equal(h.reads.length,4);resolve(h,[{note_id:'recovered'}]);await b;
  assert.equal(h.task.notes[0].note_id,'recovered');
 });
 test(name+' polling shares the note read with a selected archive load',async()=>{
  const h=harness(),a=h.load(),b=h.read();assert.equal(h.reads.length,2);
  resolve(h,[{note_id:'one'}]);await Promise.all([a,b]);assert.equal(h.task.notes[0].note_id,'one');
 });
 test(name+' failed stats preserve the last confirmed statistics',async()=>{
  const h=harness();h.task.readingStats=[{detail_notes:8}];const a=h.load();
  h.reads.find(r=>r.command==='agent_task_reading_stats').reject(Error('offline'));
  h.reads.find(r=>r.command==='agent_task_notes').resolve([{note_id:'old'}]);
  await a;assert.equal(h.task.readingStats[0].detail_notes,8);assert.equal(h.renders(),0);
 });
}
