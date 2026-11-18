// Actual native Worker/Agent loop with scripted loopback responses; no real LLM or Chrome.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {spawn} from 'node:child_process';
import {mkdtemp,writeFile,readFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {createHash,randomUUID} from 'node:crypto';
const binary=resolve(process.argv[2]),label=process.argv[3];
assert(['before','after'].includes(label));
const preloaded=process.argv[4]==='--preload-research';
assert(process.argv.length===4||(preloaded&&process.argv.length===5));
const stage=await mkdtemp('/tmp/socai-research-contract-');
const credentials=join(stage,'synthetic-auth.json'),config=join(stage,'config.json');
await writeFile(credentials,JSON.stringify({socai_pro:{device_id:'synthetic-device',device_token:'synthetic-loopback-token',user_id:'synthetic-user'}}),{mode:0o600,flag:'wx'});
await writeFile(config,'{}',{flag:'wx'});
const calls=[
 // Both are real entrypoints: quick mode loads the skill through a tool;
 // deep mode loads it before the first request and reads the initial ledger.
 {label:'activate',name:preloaded?'research_read':'read_skill',input:preloaded?{}:{name:'research'}},
 {label:'requires-initialization',name:'research_update',input:{exploration:'Synthetic initial plan'},error:/initialize research/},
 {label:'initialize',name:'research_update',input:{objective:'Synthetic transport contract',criteria:['Preserve the saved plan']}},
 {label:'save-plan',name:'research_update',input:{next_steps:['Inspect fixture A','Compare fixture B']}},
 {label:'read-plan',name:'research_read',input:{}},
 {label:'reject-string',name:'research_update',input:{next_steps:'not-an-array'},error:/invalid type/},
 {label:'reject-capacity',name:'research_update',input:{next_steps:Array(6).fill('Synthetic step')},error:/next_steps supports at most 5 entries/},
 {label:'reject-empty-step',name:'research_update',input:{next_steps:['   ']},error:/next_steps must be non-empty/},
 {label:'reject-long-step',name:'research_update',input:{next_steps:['界'.repeat(2001)]},error:/next_steps.*2000 characters/},
 {label:'reject-unknown-field',name:'research_update',input:{synthetic_unknown_field:true},error:/unknown field/},
 {label:'retain-plan',name:'research_update',input:{exploration:'An updated synthetic notebook; plan omitted'}},
 {label:'replace-plan',name:'research_update',input:{next_steps:['Finish synthetic handoff']}},
 {label:'clear-plan',name:'research_update',input:{next_steps:[]}},
 {label:'reject-empty-criteria',name:'research_update',input:{criteria:[]},error:/criteria must describe at least one deliverable/},
 {label:'reject-unacknowledged-blocker',name:'research_update',input:{review:{reason:'blocked',coverage:'Synthetic blocker',remaining_gaps:[]}},error:/remaining_gaps must acknowledge/},
 {label:'record-honest-review',name:'research_update',input:{review:{reason:'satisfied',coverage:'Synthetic local contract exercised; no external source claims',remaining_gaps:[]}}},
];
const requests=[],frames=[];let serverError=null;
const server=createServer(async(req,res)=>{
 try{
  assert.equal(req.url,'/v1/llm/chat/completions');
  assert.equal(req.headers.authorization,'Bearer synthetic-loopback-token');
  let raw='';for await(const chunk of req){raw+=chunk;assert(raw.length<500000);}
  const body=JSON.parse(raw),index=requests.length;requests.push(body);
  assert(index<=calls.length,'Unexpected extra model request');
  const call=calls[index];
  const message=call?{role:'assistant',content:null,tool_calls:[{id:'fixture-call-'+index,type:'function',function:{name:call.name,arguments:JSON.stringify(call.input)}}]}:{role:'assistant',content:'Synthetic contract complete; not a customer task.'};
  res.writeHead(200,{'content-type':'application/json'}).end(JSON.stringify({id:'fixture-response-'+index,object:'chat.completion',choices:[{index:0,message,finish_reason:call?'tool_calls':'stop'}],usage:{prompt_tokens:100,completion_tokens:10,total_tokens:110}}));
 }catch(error){serverError=error;res.writeHead(500,{'content-type':'application/json'}).end('{"error":"synthetic fixture assertion failed"}');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const port=server.address().port;
const env={PATH:process.env.PATH,TMPDIR:process.env.TMPDIR||'/tmp',
 SOCAI_SESSION_DIR:join(stage,'session'),SOCAI_HOME:join(stage,'home'),SOCAI_RUNS_DIR:join(stage,'runs'),
 SOCAI_CONFIG_PATH:config,SOCAI_CLOUD_AUTH_FILE:credentials,SOCAI_PRO_BASE_URL:`http://127.0.0.1:${port}`,
 SOCAI_TELEMETRY:'off',SOCAI_TELEMETRY_TASK_TEXT:'off',SOCAI_TELEMETRY_QUERY_TEXT:'off',RUST_LOG:'error'};
const child=spawn(binary,[],{env,stdio:['pipe','pipe','pipe']});
const binding={user_id:'synthetic-user',session_id:randomUUID(),kernel_id:'synthetic-kernel',assignment_epoch:1};
let buffer='',stderr='',sent=false,terminal=false;
const checks=[];const check=(name,ok)=>checks.push({name,ok:Boolean(ok)});
const send=value=>child.stdin.write(JSON.stringify({protocol_version:2,...value})+'\n');
child.stderr.on('data',data=>{stderr+=data;});
child.stdout.on('data',data=>{
 buffer+=data;
 for(;;){const end=buffer.indexOf('\n');if(end<0)break;const line=buffer.slice(0,end);buffer=buffer.slice(end+1);if(!line.trim())continue;
  let frame;try{frame=JSON.parse(line);}catch{continue;}frames.push(frame);
  if(frame.frame==='ready'&&!sent){sent=true;send({type:'session.bind',request_id:'fixture-bind',binding});}
  if(frame.frame==='bound')send({type:'run.start',request_id:'fixture-start',binding,run_id:'research-fixture-'+randomUUID(),prompt:'Synthetic local research contract; do not browse or contact external services.',provider:'socai',model:'fixture-model',enabled_sites:[],max_steps:30,max_tokens:1024,research:{deep:preloaded}});
  if(frame.frame==='event'&&['done','failed','cancelled'].includes(frame.event?.event)){terminal=true;send({type:'shutdown',request_id:'fixture-shutdown'});child.stdin.end();}
 }
});
const deadline=setTimeout(()=>child.kill('SIGTERM'),45000);
const exit=await new Promise(resolve=>child.once('exit',(code,signal)=>resolve({code,signal})));
clearTimeout(deadline);await new Promise(resolve=>server.close(resolve));
function objects(value){
 if(typeof value==='string'){try{return objects(JSON.parse(value));}catch{return [];}}
 if(!value||typeof value!=='object')return [];
 return [value,...Object.values(value).flatMap(objects)];
}
try{
 if(serverError)throw serverError;
 check('native worker exits normally',exit.code===0&&terminal);
 check('exact scripted loop completed',requests.length===calls.length+1);
 const results=frames.filter(frame=>frame.frame==='event'&&frame.event.event==='tool_result').map(frame=>frame.event);
 check('all scripted tools executed',results.length===calls.length);
 const saved=new Map();
 for(let i=0;i<calls.length;i++){
  const expected=calls[i],actual=results.find(result=>result.call_id==='fixture-call-'+i);
  check(expected.label+' outcome',actual&&actual.name===expected.name&&(expected.error?expected.error.test(actual.error||''):!actual.error));
  if(!actual?.error){
   const value=objects(actual?.content).find(value=>typeof value.full_record_artifact==='string');
   if(value){const path=resolve(value.full_record_artifact);assert(path.startsWith(stage+'/'));saved.set(expected.label,JSON.parse(await readFile(path,'utf8')));}
  }
 }
 const plan=['Inspect fixture A','Compare fixture B'];
 check('plan persisted',JSON.stringify(saved.get('save-plan')?.next_steps)===JSON.stringify(plan));
 check('omitted plan retained after rejected updates',JSON.stringify(saved.get('retain-plan')?.next_steps)===JSON.stringify(plan));
 check('plan replaced rather than appended',JSON.stringify(saved.get('replace-plan')?.next_steps)===JSON.stringify(['Finish synthetic handoff']));
 check('explicit empty plan clears it',JSON.stringify(saved.get('clear-plan')?.next_steps)==='[]');
 check('objective and criteria retained',saved.get('clear-plan')?.objective==='Synthetic transport contract'&&saved.get('clear-plan')?.criteria?.length===1);
 const afterActivation=JSON.stringify(requests[1]?.messages?.at(-1)||{});
 check('initialization checkpoint is explicit',afterActivation.includes('Research ledger is not initialized')&&afterActivation.includes('objective')&&afterActivation.includes('criteria'));
 const schema=requests[2]?.tools?.find(tool=>tool.function?.name==='research_update')?.function?.parameters;
 check('schema advertises bounded plan',schema?.properties?.next_steps?.type==='array'&&schema.properties.next_steps.maxItems===5&&schema.properties.next_steps.items.maxLength===2000&&schema.additionalProperties===false);
 const checkpoint=requests[4]?.messages?.at(-1);
 check('plan is visible in later model checkpoint',checkpoint?.role==='user'&&JSON.stringify(checkpoint.content).includes('Runtime research checkpoint')&&JSON.stringify(checkpoint.content).includes('Inspect fixture A'));
 const read=results.find(result=>result.call_id==='fixture-call-4');
 check('research_read returns current plan',objects(read?.content).some(value=>JSON.stringify(value.next_steps)===JSON.stringify(plan)));
}catch(error){checks.push({name:'fixture validation',ok:false,error:error.message});}
const result={at:new Date().toISOString(),binary,label,stage,exit,researchActivation:preloaded?'entrypoint-preloaded':'read-skill-tool',checks,allVerified:checks.every(check=>check.ok),httpRequests:requests.length,
 actualNativeWorker:true,scriptedLoopbackModel:true,paidModelCalls:0,chromeTouched:false,customerTasksReplayed:0,productionAuthUsed:false,installedCliChanged:false,
 technicalErrors:frames.filter(frame=>frame.event?.event==='tool_result'&&frame.event.error).map(frame=>({callId:frame.event.call_id,error:frame.event.error})),
 terminalKind:frames.find(frame=>['done','failed','cancelled'].includes(frame.event?.event))?.event?.event||null,
 stderrTail:stderr.slice(-2000)};
await writeFile(join(stage,'result.json'),JSON.stringify(result,null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify(result));if(!result.allVerified)process.exitCode=1;
