import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
async function setup(deferred=false){
 const source=await readFile(new URL('../src/lib/voice-input.ts',import.meta.url),'utf8');
 const activity=await readFile(new URL('../src/lib/voice-activity.ts',import.meta.url),'utf8');
 let processor,resolveResult;const calls=[],callbacks=[];let stopped=0;
 class AudioContext {
  sampleRate=16000;state='running';destination={};
  createMediaStreamSource(){return{connect(){},disconnect(){}};}
  createScriptProcessor(){return processor={connect(){},disconnect(){},onaudioprocess:null};}
  createGain(){return{gain:{value:0},connect(){},disconnect(){}};}
  async resume(){} async close(){this.state='closed';}
 }
 const context={AudioContext,navigator:{mediaDevices:{async getUserMedia(){return{getTracks:()=>[{stop(){stopped++;}}]};}}},
  document:{querySelectorAll:()=>[]},window:{setTimeout:()=>1,clearTimeout(){},setInterval:()=>2,clearInterval(){}},
  DOMException,Float32Array,Uint8Array,ArrayBuffer,DataView,console:{error(){}},btoa:s=>Buffer.from(s,'binary').toString('base64'),
  t:key=>key,getLanguage:()=> 'zh',prepareVoiceTranscript(){},normalizeVoiceTranscript:async text=>text,
  listen:async()=>()=>{},invoke:async(name,args)=>{
   if(name==='whisper_status')return{ready:true,state:'ready',model_size:'medium'};
   if(name==='voice_input_status')return{ready:true,route:'cloud',state:'cloud_ready',local_state:'not_checked'};
   calls.push({name,args});
   return deferred?new Promise(r=>{resolveResult=r;}):{text:'真实处理器单元测试'};
  }};
 // Node's strip-only API cannot lower namespaces. Preserve the actual capture
 // implementation in a lexical block and expose only its public test surface.
 const implementation=source.replace(/^import .*;\n/gm,'')
  .replace('export namespace voiceInput {','{')
  .replace(/\n}\n\nfunction (mergeChunks|downloadPercent)/,
   '\nglobalThis.voice={initialize,toggle,cancelRecording,composerState};\n}\n\nfunction $1')
  .replaceAll('export ','');
 const script=activity.replaceAll('export ','')+'\n'+implementation;
 runInNewContext(stripTypeScriptTypes(script),context);
 await context.voice.initialize(()=>{});
 await context.voice.toggle(text=>callbacks.push(text));
 const frame=(amplitude=0)=>processor.onaudioprocess?.({inputBuffer:{getChannelData:()=>Float32Array.from({length:2048},(_,i)=>Math.sin(i*0.2)*amplitude)},outputBuffer:{getChannelData:()=>new Float32Array(2048)}});
 return{voice:context.voice,calls,callbacks,frame,resolve:()=>resolveResult?.({text:'迟到文字'}),stopped:()=>stopped};
}
const flush=()=>new Promise(resolve=>setImmediate(resolve));
test('real capture handler auto-finishes once after speech and inserts without a second click',async()=>{
 const s=await setup();for(let i=0;i<4;i++)s.frame(0.1);for(let i=0;i<12;i++)s.frame();
 await flush();assert.equal(s.calls.length,1);assert.deepEqual(s.callbacks,['真实处理器单元测试']);assert.ok(s.stopped()>0);
 assert.equal(s.voice.composerState().phase,'idle');
});
test('initial empty capture stops the microphone without an upstream request',async()=>{
 const s=await setup();for(let i=0;i<94;i++)s.frame();await flush();
 assert.equal(s.calls.length,0);assert.equal(s.voice.composerState().phase,'idle');assert.equal(s.voice.composerState().error,'voice.error.noSpeech');
});
test('manual stop and automatic silence cannot submit the same capture twice',async()=>{
 const s=await setup(true);for(let i=0;i<4;i++)s.frame(0.1);for(let i=0;i<12;i++)s.frame();
 await s.voice.toggle();assert.equal(s.calls.length,1);s.resolve();await flush();assert.equal(s.callbacks.length,1);
});
test('leaving while transcribing discards a late result and releases UI busy state',async()=>{
 const s=await setup(true);for(let i=0;i<4;i++)s.frame(0.1);for(let i=0;i<12;i++)s.frame();
 s.voice.cancelRecording();s.resolve();await flush();assert.equal(s.callbacks.length,0);assert.equal(s.voice.composerState().phase,'idle');
});
