// Exercise the actual native CLI over task-only fake Unix sockets, never Chrome.
import assert from 'node:assert/strict';
import {createServer,createConnection} from 'node:net';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtemp,readFile,lstat,writeFile} from 'node:fs/promises';
import {join,resolve} from 'node:path';
const binary=resolve(process.argv[2]),label=process.argv[3];
assert(['before','after'].includes(label));
assert.notEqual(process.platform,'win32','This acceptance check requires Unix sockets');
const stage=await mkdtemp('/tmp/socai-ipc-check-');
const scenarios=['eof','malformed','missing-result','legacy-success','application-error','success',
 ...(label==='after'?['progress-eof','cold-start','unconfirmed-shutdown','rejected-shutdown','shutdown-eof','shutdown-timeout','confirmed-version-shutdown','confirmed-build-shutdown','confirmed-delayed-version-shutdown']:[])];
const rows=[];
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
for(const scenario of scenarios){
 const {mkdir}=await import('node:fs/promises');
 const home=join(stage,scenario);await mkdir(home);
 const socket=join(home,'rust-daemon.sock');let requests=0,shutdowns=0;
 const confirmedShutdown=scenario.startsWith('confirmed-');
 const rejectedShutdown=['unconfirmed-shutdown','rejected-shutdown','shutdown-eof','shutdown-timeout'].includes(scenario);
 const server=createServer(s=>{
  let text='';s.on('error',()=>{});s.on('data',data=>{
   text+=data;if(!text.includes('\n'))return;
   const request=JSON.parse(text.split('\n')[0]);text='';
   if(request.command==='shutdown'){
    shutdowns++;
    if(scenario==='shutdown-timeout')return; // Keep IPC open past the real shutdown deadline.
    if(scenario==='shutdown-eof'){s.end();return;}
    if(scenario==='rejected-shutdown'){
     s.end(JSON.stringify({id:request.id,ok:false,error:'Intentional shutdown rejection'})+'\n');return;
    }
    s.end(JSON.stringify({id:request.id,ok:true,result:{ok:true}})+'\n');
    if(confirmedShutdown){
     if(scenario==='confirmed-delayed-version-shutdown')setTimeout(()=>server.close(),3000);
     else server.close();
    }
    return;
   }
   assert.equal(request.command,'__task_begin','Only task registration may reach this fake daemon');requests++;
   const frame={id:request.id,ok:true,version:request.version,build_id:request.build_id,result:{verified:true}};
   if(scenario==='eof'){s.end();return;}
   if(scenario==='progress-eof'){
    s.end(JSON.stringify({type:'progress',id:request.id,event:{phase:'reading',status:'item_started',current:0,total:1}})+'\n');return;
   }
   if(scenario==='malformed'){s.end('not-json\n');return;}
   if(scenario==='missing-result')delete frame.result;
   if(scenario==='legacy-success'){delete frame.version;delete frame.build_id;}
   if(scenario==='application-error'){frame.ok=false;delete frame.result;frame.error='Intentional browser error; keep the transport';}
   if(rejectedShutdown){frame.ok=false;delete frame.result;frame.code='version-mismatch';frame.error='Intentional version rejection before execution';}
   if(confirmedShutdown){frame.ok=false;delete frame.result;frame.code=scenario==='confirmed-build-shutdown'?'stale-daemon':'version-mismatch';frame.error='Intentional rejection before execution; shutdown may proceed';}
   s.end(JSON.stringify(frame)+'\n');
  });
 });
 if(scenario!=='cold-start')await new Promise((r,j)=>{server.once('error',j);server.listen(socket,r)});
 const inode=(await lstat(socket).catch(()=>null))?.ino??null;
 const started=performance.now();
 const child=spawn(binary,['task','begin','--context-file','-'],{env:{...process.env,SOCAI_HOME:home,SOCAI_RUNS_DIR:join(home,'runs'),SOCAI_TELEMETRY:'off',SOCAI_TELEMETRY_TASK_TEXT:'off'},stdio:['pipe','pipe','pipe']});
 child.stdin.end(JSON.stringify({user_prompt:null,agent_host:'transport-check'}));
 let stdout='',stderr='';child.stdout.on('data',d=>stdout+=d);child.stderr.on('data',d=>stderr+=d);
 const timer=setTimeout(()=>child.kill('SIGTERM'),20000);
 const exit=await new Promise(r=>child.once('exit',r));clearTimeout(timer);
 const elapsedMs=Math.round(performance.now()-started);
 if(scenario==='confirmed-delayed-version-shutdown'&&exit===0)assert(elapsedMs>=3000,'The successor must wait for the old endpoint to be released');
 if(scenario==='shutdown-timeout')assert(elapsedMs>=9500&&elapsedMs<15000,'Exercise the native ten-second shutdown timeout, not the fixture kill deadline');
 const current=await lstat(socket).catch(()=>null),replaced=(current?.ino??null)!==inode;
 // A broken baseline can launch a replacement in this unique test home.
 // Stop only that verified child by its own IPC, not the global `socai stop` sweep.
 let testDaemonStopped=false;
 if(replaced){
  const pid=Number(await readFile(join(home,'rust-daemon.pid'),'utf8'));
  assert(Number.isSafeInteger(pid)&&pid>1);
  const command=execFileSync('/bin/ps',['-p',String(pid),'-o','command='],{encoding:'utf8'}).trim();
  assert.equal(command,binary+' __daemon');
  await new Promise((r,j)=>{
   const s=createConnection(socket);s.setTimeout(4000,()=>s.destroy(Error('Test shutdown timed out')));
   s.once('error',j);s.once('connect',()=>s.write(JSON.stringify({id:'isolated-shutdown',site:'',command:'shutdown',args:{}})+'\n'));
   s.once('data',()=>{s.end();r()});
  });
  for(let i=0;i<40;i++){
   if(!(await lstat(join(home,'rust-daemon.pid')).catch(()=>null))){testDaemonStopped=true;break;}
   await sleep(100);
  }
  assert(testDaemonStopped,'Inspect the isolated test daemon before doing anything else');
 }
 if(server.listening)await new Promise(r=>server.close(r));
 const expectedExit=['success','cold-start'].includes(scenario)||confirmedShutdown?0:1;
 const noReplay=scenario==='cold-start'?requests===0&&replaced&&testDaemonStopped&&exit===0:
  confirmedShutdown?requests===1&&shutdowns===1&&replaced&&testDaemonStopped&&exit===0:
  !replaced&&requests===1&&shutdowns===(rejectedShutdown?1:0)&&exit===expectedExit;
 rows.push({scenario,requests,shutdowns,replaced,exit,elapsedMs,noReplay,testDaemonStopped,
  errorClass:/outcome is unknown/.test(stderr)?'outcome-unknown':/empty daemon response/.test(stderr)?'eof':/Intentional browser error/.test(stderr)?'application-error':null,
  returnedSuccess:stdout.trim().startsWith('{')});
}
const result={at:new Date().toISOString(),binary,label,stage,realNativeBinary:true,chromeTouched:false,telemetryEnabled:false,rows,allVerified:rows.every(r=>r.noReplay)};
await writeFile(join(stage,'result.json'),JSON.stringify(result,null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify(result));
if(!result.allVerified)process.exitCode=1;
