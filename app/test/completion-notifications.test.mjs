import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import vm from 'node:vm';

const source=stripTypeScriptTypes(await readFile(new URL('../src/lib/completion-notifications.ts',import.meta.url),'utf8'))
  .replace(/^import .*;\n/gm,'').replace(/^export /gm,'');
const boot=1795021200000;
function fixture({stored=null,focused=false,visible=true,granted=true,permissionError=false,storeBlocked=false}={}) {
  const sent=[],warnings=[];let requests=0;
  const context=vm.createContext({Set,Date:{now:()=>boot},console:{warn:(...args)=>warnings.push(args)},
    document:{visibilityState:visible?'visible':'hidden',hasFocus:()=>focused},
    localStorage:{getItem:()=>{if(storeBlocked)throw Error('blocked');return stored;},setItem:()=>{if(storeBlocked)throw Error('blocked');}},
    isPermissionGranted:async()=>{if(permissionError)throw Error('permission unavailable');return granted;},
    requestPermission:async()=>{requests++;return 'granted';},sendNotification:options=>sent.push(options),getLanguage:()=> 'zh'});
  vm.runInContext(source,context);
  const run=code=>vm.runInContext(code,context);
  return {sent,warnings,requests:()=>requests,enabled:()=>run('completionNotificationsEnabled()'),
    set:value=>run(`setCompletionNotificationsEnabled(${value})`),request:()=>run('requestCompletionNotificationPermission()'),
    notify:event=>{context.event=event;return run('notifyTaskCompletion(event)');}};
}
const completed=(run='run-1')=>({task_id:'task-1',kind:'completed',created_at:boot+1000,
  snapshot:{status:'completed',run_id:run,finished_at:boot+1000}});
test('completion alerts default on, explicit opt-out and blocked storage are respected',()=>{
  assert.equal(fixture().enabled(),true);assert.equal(fixture({stored:'false'}).enabled(),false);
  const f=fixture({storeBlocked:true});f.set(false);assert.equal(f.enabled(),false);
});
test('only a committed completed snapshot notifies, not raw done, failed, cancelled or interrupted',async()=>{
  const f=fixture();
  for(const kind of ['done','failed','cancelled','interrupted'])await f.notify({...completed(),kind});
  for(const status of ['failed','cancelled','interrupted','running'])await f.notify({...completed(),snapshot:{...completed().snapshot,status}});
  assert.equal(f.sent.length,0);await f.notify(completed());assert.equal(f.sent.length,1);
});
test('duplicates notify once while follow-up runs can notify separately',async()=>{
  const f=fixture();await Promise.all([f.notify(completed()),f.notify(completed())]);
  assert.equal(f.sent.length,1);await f.notify(completed('run-2'));assert.equal(f.sent.length,2);
});
test('historical completions and missing run identity never notify',async()=>{
  const f=fixture();await f.notify({...completed(),snapshot:{...completed().snapshot,finished_at:boot-1}});
  await f.notify({...completed(),snapshot:{...completed().snapshot,run_id:null}});assert.equal(f.sent.length,0);
});
test('foreground completions stay quiet and are not replayed after hiding',async()=>{
  const f=fixture({focused:true});await f.notify(completed());assert.equal(f.sent.length,0);
});
test('denied permissions and explicit opt-out remain quiet without task retries',async()=>{
  for(const options of [{granted:false},{stored:'false'},{permissionError:true}]) {
    const f=fixture(options);await f.notify(completed());assert.equal(f.sent.length,0);assert.equal(f.requests(),0);
  }
});
test('the permission request is explicit and the notification contains no prompt or identity',async()=>{
  const f=fixture({granted:false});assert.equal(f.requests(),0);f.request();await new Promise(r=>setImmediate(r));assert.equal(f.requests(),1);
  const g=fixture();await g.notify({...completed(),text:'private prompt',email:'private@example.com'});
  assert.deepEqual(Object.keys(g.sent[0]).sort(),['body','title']);assert(!JSON.stringify(g.sent).includes('private'));
});
