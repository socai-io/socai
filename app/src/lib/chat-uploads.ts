import {invoke} from "@tauri-apps/api/core";
import {esc} from "./html";
import {getLanguage} from "./i18n";
import {artifactFileIcon, formatArtifactSize} from "../panels/artifact_preview";

export type ChatUpload = {id:string; name:string; size:number; sha256:string};
const drafts = new Map<string,ChatUpload[]>();
const errors = new Map<string,string>();
let picking: string | null = null;
const tr = (zh:string,en:string) => getLanguage()==="zh" ? zh : en;
const key = (scope:string) => `socai.desktop-uploads.v1:${scope}`;
export function uploadDraft(scope:string):ChatUpload[] {
  if (!drafts.has(scope)) {
    let items:ChatUpload[]=[];
    try {
      const stored:unknown=JSON.parse(localStorage.getItem(key(scope))||"[]");
      if(Array.isArray(stored)) items=stored.filter(file=>file && typeof file.id==="string" && typeof file.name==="string" && typeof file.size==="number").slice(0,10);
    } catch { /* Draft storage may be unavailable. */ }
    drafts.set(scope,items);
  }
  return drafts.get(scope)!;
}
function persist(scope:string,items:ChatUpload[]):void {
  drafts.set(scope,items);
  try { localStorage.setItem(key(scope),JSON.stringify(items)); } catch { /* Memory draft still works. */ }
}
export const uploadIds=(scope:string):string[]=>uploadDraft(scope).map(file=>file.id);
export const uploadsBusy=(scope:string):boolean=>picking===scope;
export function clearUploads(scope:string):void { persist(scope,[]);errors.delete(scope); }

export function renderUploads(files:ChatUpload[],scope?:string,disabled=false):string {
  return `<div class="chat-upload-list" aria-live="polite">${files.map(file=>`<div class="chat-upload-card">
    <span class="chat-upload-icon" aria-hidden="true">${artifactFileIcon(file.name)}</span>
    <span class="chat-upload-info"><span title="${esc(file.name)}">${esc(file.name)}</span><small>${esc(formatArtifactSize(file.size))}</small></span>
    <button type="button" class="chat-upload-download" data-chat-upload-save="${esc(file.id)}" title="${tr("下载原件","Save original")}" aria-label="${esc(tr("下载原件：","Save original: ")+file.name)}"><svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M12 3v12m-5-5 5 5 5-5M5 16v4h14v-4"/></svg></button>
    ${scope?`<button type="button" data-chat-upload-remove="${esc(file.id)}" aria-label="${esc(tr("移除：","Remove: ")+file.name)}" ${disabled?"disabled":""}>×</button>`:""}
  </div>`).join("")}${scope&&uploadsBusy(scope)?`<span role="status">${tr("正在读取文件…","Reading files…")}</span>`:""}${scope&&errors.get(scope)?`<p role="alert">${esc(errors.get(scope)!)}</p>`:""}</div>`;
}
export function bindUploads(scope:string,changed:()=>void):void {
  document.getElementById("composer-attach")?.addEventListener("click",async()=>{
    if(picking!==null)return;
    picking=scope;errors.delete(scope);changed();
    try {
      const files=await invoke<ChatUpload[]>("chat_upload_pick");
      const next=[...uploadDraft(scope),...files];
      if(next.length>10)throw new Error(tr("每次最多发送10个文件，请先移除部分文件。","Send up to 10 files; remove some first."));
      persist(scope,next);
    } catch(error) {errors.set(scope,String(error));}
    finally {picking=null;changed();}
  });
  document.querySelectorAll<HTMLButtonElement>("[data-chat-upload-remove]").forEach(button=>button.addEventListener("click",()=>{
    persist(scope,uploadDraft(scope).filter(file=>file.id!==button.dataset.chatUploadRemove));errors.delete(scope);changed();
  }));
  document.querySelectorAll<HTMLButtonElement>("[data-chat-upload-save]").forEach(button=>button.addEventListener("click",async()=>{
    button.disabled=true;
    try {await invoke("chat_upload_save",{id:button.dataset.chatUploadSave});}
    catch(error){errors.set(scope,String(error));changed();}
    finally{button.disabled=false;}
  }));
}
