// Local fault-injection acceptance with real Chrome, native search, agent loop
// and the configured model. No production pages or user browser profile used.
// Usage: cargo build -p socai-core --example check_browser_recovery
//        node scripts/check-browser-recovery.mjs [evidence-dir]
import {spawn} from 'node:child_process';
import {mkdtemp, readFile, rm, writeFile, mkdir} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const repo=fileURLToPath(new URL('..',import.meta.url));
const dir=process.argv[2]?resolve(process.argv[2]):await mkdtemp(tmpdir()+'/socai-recovery-check-');
await mkdir(dir,{recursive:true});
const profile=await mkdtemp(tmpdir()+'/socai-recovery-chrome-');
const executable=process.env.CHROME_BIN || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const chrome=spawn(executable,['--headless=new','--no-first-run','--disable-background-networking','--remote-debugging-port=0','--user-data-dir='+profile,'about:blank'],{stdio:'ignore'});
let socket,child;
try {
 let port;
 for(let n=0;n<100;n++){try{port=Number((await readFile(profile+'/DevToolsActivePort','utf8')).split('\n')[0]);break;}catch{await new Promise(r=>setTimeout(r,100));}}
 assert(port,'isolated Chrome did not start');
 const version=await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
 socket=new WebSocket(version.webSocketDebuggerUrl);await new Promise((r,j)=>{socket.onopen=r;socket.onerror=j;});
 let seq=0;const pending=new Map();
 const call=(method,params={},sessionId)=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));});
 const padding=Array.from({length:3200},(_,i)=>`<span class="padding-row" data-row="${i}">·</span>`).join('');
 const html=`<!doctype html><html><head><style>/*RAW_STYLE_SENTINEL*/.padding-row{display:none}</style></head><body><h1>小红书搜索</h1>${padding}<form id="search-form"><input id="search-v2" placeholder="搜索笔记"><button id="submit-v2" type="submit">搜索</button></form><main id="results"></main><footer>DOM_TAIL_SENTINEL</footer><script>/*RAW_SCRIPT_SENTINEL${'x'.repeat(4*1024*1024)}*/
 localStorage.setItem('loads',String(Number(localStorage.getItem('loads')||0)+1));
 const render=()=>{document.querySelector('#results').innerHTML='<article class="note-item" data-note-id="111111111111111111111111"><a class="note-link" href="https://www.xiaohongshu.com/explore/111111111111111111111111?xsec_token=fixture-one">晨间咖啡</a></article><article class="note-item" data-note-id="222222222222222222222222"><a class="note-link" href="https://www.xiaohongshu.com/explore/222222222222222222222222?xsec_token=fixture-two">手冲入门</a></article>';};
 document.querySelector('#search-form').addEventListener('submit',e=>{e.preventDefault();localStorage.setItem('ui_submits',String(Number(localStorage.getItem('ui_submits')||0)+1));history.pushState(null,'','/search_result?keyword='+encodeURIComponent(document.querySelector('#search-v2').value));render();});
 if(location.pathname==='/search_result'){document.querySelector('#search-v2').value=new URL(location.href).searchParams.get('keyword')||'';render();}
 </script></body></html>`;
 socket.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const p=pending.get(m.id);pending.delete(m.id);m.error?p.reject(Error(m.error.message)):p.resolve(m.result);}else if(m.method==='Fetch.requestPaused'){
   // Every request from the owned test page is fulfilled locally, even if a
   // model attempts another URL. There is no production-site network traffic.
   call('Fetch.fulfillRequest',{requestId:m.params.requestId,responseCode:200,responseHeaders:[{name:'Content-Type',value:'text/html; charset=utf-8'}],body:Buffer.from(html).toString('base64')},m.sessionId).catch(console.error);
 }};
 await mkdir(dir+'/site-skills/xhs',{recursive:true});
 const manifest=JSON.parse(await readFile(repo+'/core/src/sites/xhs/manifest.json','utf8'));manifest.notes=[];
 await writeFile(dir+'/site-skills/xhs/manifest.json',JSON.stringify(manifest));
 await writeFile(dir+'/site-skills/xhs/page_scripts.js',`const SocaiXhsPageScripts=(()=>({
 loginState:()=>({login:'in'}),
 searchInput:()=>{const n=Number(localStorage.getItem('attempts')||0)+1;localStorage.setItem('attempts',String(n));if(localStorage.getItem('scenario')==='persistent'||n===1)return {ok:false,reason:'search_input_not_found',page_ocr_text:'Search input unavailable'};return {ok:true};},
 selectSearchInput:()=>{const el=document.querySelector('#search-v2');el.focus();el.select();return {ok:true};},
 setSearchInput:arg=>{document.querySelector('#search-v2').value=arg.query;return {ok:true};},
 searchState:()=>({url:location.href,page_state:location.pathname==='/search_result'?'search_results':'home',input_keyword:document.querySelector('#search-v2').value,url_keyword:new URL(location.href).searchParams.get('keyword')||'',card_count:document.querySelectorAll('.note-item').length,login_required:false}),
 searchCards:()=>Array.from(document.querySelectorAll('.note-item'),(el,i)=>({note_id:el.dataset.noteId,title:el.textContent,author:'本地测试',likes:'1',link:el.querySelector('a').href,xsec_token:new URL(el.querySelector('a').href).searchParams.get('xsec_token'),position:i})),
 }))();`);
 await rm(dir+'/target',{force:true});await rm(dir+'/ready',{force:true});
 const env={...process.env,SOCAI_CDP_WS:version.webSocketDebuggerUrl,SOCAI_SITE_SKILLS_DIR:dir+'/site-skills',SOCAI_TELEMETRY:'0'};
 child=spawn(repo+'/target/debug/examples/check_browser_recovery',[dir],{env,stdio:['ignore','pipe','pipe']});
 let output='';child.stdout.on('data',b=>{output+=b;process.stdout.write(b);});child.stderr.on('data',b=>process.stderr.write(b));
 const exit=new Promise(r=>child.once('exit',r));
 let target;for(let n=0;n<250;n++){try{target=await readFile(dir+'/target','utf8');break;}catch{if(child.exitCode!==null)throw Error('helper exited before browser setup');await new Promise(r=>setTimeout(r,100));}}
 assert(target,'helper did not provide an owned test target');
 const {sessionId}=await call('Target.attachToTarget',{targetId:target,flatten:true});
 await call('Fetch.enable',{patterns:[{urlPattern:'*'}]},sessionId);await writeFile(dir+'/ready','1');
 const status=await exit;
 await writeFile(dir+'/execution.jsonl',output);
 console.log('Evidence: '+dir);
 assert.equal(status,0,'recovery acceptance failed; inspect evidence');
} finally {
 child?.kill();socket?.close();chrome.kill('SIGTERM');
 if(chrome.exitCode===null)await new Promise(r=>chrome.once('exit',r));
 await rm(profile,{recursive:true,force:true});
}
