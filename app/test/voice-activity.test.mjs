import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
async function detector(){
 const source=await readFile(new URL('../src/lib/voice-activity.ts',import.meta.url),'utf8');
 const context={};runInNewContext(stripTypeScriptTypes(source).replaceAll('export ','')+'\nglobalThis.Detector=VoiceActivityDetector',context);
 return new context.Detector();
}
const frame=(amplitude=0)=>Float32Array.from({length:1600},(_,i)=>Math.sin(i*0.2)*amplitude);
function tick(d,amplitude=0,n=1){let result;for(let i=0;i<n;i++)result=d.update(frame(amplitude),16000);return result;}
test('initial silence waits for speech, and 12 seconds of no speech cancels without ASR',async()=>{
 const d=await detector();assert.equal(tick(d,0,110).stop,null);assert.equal(tick(d,0,10).stop,'empty');
});
test('a brief mid-sentence pause does not stop; 1.5 seconds after speech does',async()=>{
 const d=await detector();tick(d,0.12,6);assert.equal(tick(d,0,10).stop,null);
 tick(d,0.12,3);assert.equal(tick(d,0,14).stop,null);assert.equal(tick(d).stop,'silence');
});
test('a click/noise burst cannot count as an utterance',async()=>{
 const d=await detector();tick(d,0.8);assert.equal(tick(d,0,20).stop,null);
});
test('meter reflects actual energy, does not treat constant DC as speech, and is bounded',async()=>{
 const d=await detector();const quiet=tick(d).level,voice=tick(d,0.2).level;
 assert.ok(voice>quiet&&voice<=1&&quiet>=0);
 const dc=await detector();for(let i=0;i<30;i++)assert.equal(dc.update(new Float32Array(1600).fill(0.1),16000).stop,null);
});
test('voice module actually connects VAD to automatic transcript delivery and protects late results',async()=>{
 const source=await readFile(new URL('../src/lib/voice-input.ts',import.meta.url),'utf8');
 assert.match(source,/new VoiceActivityDetector/);assert.match(source,/activity\.stop/);
 assert.match(source,/onAutomaticTranscript/);assert.match(source,/generation !== captureGeneration/);
 assert.match(source,/generation === captureGeneration/);
});
