// Local regression only. Reuse approved daily Chrome; never launch Chrome.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {homedir} from 'node:os';
import {pathToFileURL} from 'node:url';
import {readFile,writeFile} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
const [output, existingTarget] = process.argv.slice(2);
assert.ok(output,'Usage: node scripts/check-xhs-comment-dom.mjs /absolute/new-evidence.json');
assert.ok(output.startsWith('/'),'Evidence path must be absolute');
const initiator=process.env.SOCAI_ACCEPTANCE_TARGET;
const expectedEmail=process.env.SOCAI_ACCEPTANCE_EMAIL;
const expectedId=process.env.SOCAI_ACCEPTANCE_ACCOUNT_ID;
assert.ok(initiator && expectedEmail && expectedId,
  'SOCAI_ACCEPTANCE_TARGET, SOCAI_ACCEPTANCE_EMAIL and SOCAI_ACCEPTANCE_ACCOUNT_ID are required');
const {command} = await import(pathToFileURL(`${homedir()}/Library/Application Support/MasakaCDP/runtime/src/client.mjs`));
const script=await readFile(new URL('../core/src/sites/xhs/page_scripts.js',import.meta.url));
const fixture=await readFile(new URL('../core/src/sites/xhs/fixtures/comment-pagination.html',import.meta.url));
const pathname='/comment-dom-'+randomUUID();
const server=createServer((req,res)=>{
  if(req.url===pathname){res.writeHead(200,{'Content-Type':'text/html; charset=utf-8','Cache-Control':'no-store'});res.end(fixture);}
  else if(req.url==='/page_scripts.js'){res.writeHead(200,{'Content-Type':'text/javascript; charset=utf-8','Cache-Control':'no-store'});res.end(script);}
  else {res.writeHead(404);res.end();}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const url=`http://127.0.0.1:${server.address().port}${pathname}`;
let targetId;
try {
  const identify=await command({action:'eval',client:'daily',target:initiator,expression:`(async()=>{
    if(location.origin!=='https://agent-dev.socai.io')throw Error('Unexpected origin');
    const r=await fetch('/api/server/v1/web/auth/me?include_quota=false',{credentials:'include',cache:'no-store',signal:AbortSignal.timeout(12000)});
    if(!r.ok)throw Error('Account check failed');const a=await r.json();
    return {email:a.email,accountId:a.account_id};})()`});
  assert.equal(identify.value?.email,expectedEmail);
  assert.equal(identify.value?.accountId,expectedId);
  const owner=await command({action:'cdp',client:'daily',method:'Target.getTargetInfo',params:{targetId:initiator}});
  // window.open inherits the authenticated initiator's Chrome context; the
  // default Target.createTarget context can be a different, anonymous profile.
  if (existingTarget) {
    const info=await command({action:'cdp',client:'daily',method:'Target.getTargetInfo',params:{targetId:existingTarget}});
    assert.match(info.targetInfo.url,/^http:\/\/127\.0\.0\.1:\d+\/comment-dom-[a-f0-9-]+$/);
    assert.equal(info.targetInfo.browserContextId,owner.targetInfo.browserContextId);
    assert.equal(info.targetInfo.openerId,initiator);
    await command({action:'cdp',client:'daily',target:existingTarget,method:'Page.navigate',params:{url}});
  } else {
    const opened=await command({action:'cdp',client:'daily',target:initiator,method:'Runtime.evaluate',params:{returnByValue:true,userGesture:true,expression:`(()=>{
      if(location.origin!=='https://agent-dev.socai.io')throw Error('Unexpected origin');
      return {opened:!!window.open(${JSON.stringify(url)},'_blank')};})()`}});
    assert.equal(opened.result?.value?.opened,true);
  }
  let page;
  for(let attempt=0;attempt<15;attempt++) {
    const pages=await command({action:'list',client:'daily'});
    page=pages.find(page=>page.url===url && (!existingTarget || page.targetId===existingTarget));
    if(page)break;
    await new Promise(resolve=>setTimeout(resolve,200));
  }
  assert.ok(page,'Fixture target not found; do not repeat window.open automatically');
  assert.equal(page.browserContextId,owner.targetInfo.browserContextId,'Fixture must inherit the authenticated account context');
  targetId=page.targetId;
  // Chrome suppresses native scroll events in a hidden tab. Foreground only
  // this verified local fixture; do not replace events with manual dispatch.
  await command({action:'cdp',client:'daily',target:targetId,method:'Page.bringToFront'});
  let result;
  // Background tabs can throttle fixture timers; observation timeout does not
  // mean the page run stopped. Keep the same target and poll its existing run.
  const deadline=Date.now()+90000;
  do {
    const reading=await command({action:'eval',client:'daily',target:targetId,expression:`(()=>{
      if(location.href!==${JSON.stringify(url)})throw Error('Fixture target changed');
      return window.__commentDOM || null;})()`});
    result=reading.value;
    if(result?.status!=='running' && result)break;
    await new Promise(resolve=>setTimeout(resolve,200));
  } while(Date.now()<deadline);
  const evidence={checkedAt:new Date().toISOString(),browser:'existing daily Chrome',account:identify.value,
    browserContextId:page.browserContextId,targetId,
    scriptSha256:createHash('sha256').update(script).digest('hex'),
    fixtureSha256:createHash('sha256').update(fixture).digest('hex'),result};
  await writeFile(output,JSON.stringify(evidence,null,2)+'\n',{flag:'wx'});
  console.log(JSON.stringify(evidence));
  assert.equal(result?.status,'passed');
} finally {
  await new Promise(resolve=>{server.close(resolve);server.closeAllConnections();});
}
