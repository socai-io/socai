//! Xiaohongshu image-note publishing through the rendered creator website.
//!
//! Files are attached with CDP's standard file-input command, fields are filled
//! with trusted keyboard input, and the final control is clicked exactly once
//! after a durable action receipt reserves the attempt.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cdp::PageSession;
use crate::sites::actions::{
    ActionActor, ActionPreview, ActionReceipt, ActionStore, ActionTarget, SocialActionKind,
    SocialActionStatus,
};

const CREATOR_URL: &str = "https://creator.xiaohongshu.com/publish/publish?source=official";
const CREATOR_IMAGE_URL: &str =
    "https://creator.xiaohongshu.com/publish/publish?source=official&target=image";
const NOTE_MANAGER_URL: &str = "https://creator.xiaohongshu.com/new/note-manager?source=official";
const TITLE_SELECTOR: &str = "input[placeholder*='标题']";
const BODY_SELECTOR: &str = "[contenteditable='true'].ProseMirror";
const IMAGE_INPUT_SELECTOR: &str = "input[type='file'][accept*='.jpg']";
const MAX_IMAGES: usize = 18;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

pub async fn prepare_publish(
    page: Arc<PageSession>,
    media: Vec<String>,
    title: String,
    text: String,
) -> anyhow::Result<Value> {
    let title = title.trim().to_string();
    let text = text.trim().to_string();
    validate_copy(&title, &text)?;
    let media = inspect_media(&media)?;

    ensure_creator_open(&page).await?;
    let initial = wait_for_creator(&page, Duration::from_secs(30)).await?;
    let actor = actor_from_state(&initial)?;
    let target_id = publish_target_id(&title, &text, &media);
    let store = ActionStore::open_default();
    let idempotency_key = format!("xhs:publish:{target_id}");
    let action_id = ActionStore::action_id_for(&idempotency_key)?;
    let mut existing_draft = false;
    if let Some(receipt) = store.load_optional(&action_id)? {
        verify_requested_intent(&receipt, &actor, &target_id, &title, &text, &media)?;
        match receipt.status() {
            SocialActionStatus::Draft => existing_draft = true,
            SocialActionStatus::Prepared => {
                let still_valid = Utc::now() <= receipt.expires_at()
                    && verify_staged_media(&receipt).is_ok()
                    && creator_state(&page)
                        .await
                        .and_then(|state| verify_prepared_receipt_state(&state, &receipt))
                        .is_ok();
                if still_valid {
                    return Ok(json!({
                        "ok": true,
                        "status": "prepared",
                        "action_id": receipt.action_id(),
                        "idempotent_replay": true,
                        "next": format!("socai xhs commit-action --action-id {}", receipt.action_id()),
                        "receipt": receipt,
                    }));
                }
                store.reset_prepared(receipt.action_id(), &actor.id, &target_id)?;
                existing_draft = true;
            }
            SocialActionStatus::Committing | SocialActionStatus::CommitUnknown => {
                return Ok(json!({
                    "ok": false,
                    "status": "commit_unknown",
                    "action_id": receipt.action_id(),
                    "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                    "receipt": receipt,
                }));
            }
            SocialActionStatus::Committed | SocialActionStatus::Reconciled => {
                return Ok(json!({
                    "ok": true,
                    "status": receipt.status(),
                    "action_id": receipt.action_id(),
                    "idempotent_replay": true,
                    "receipt": receipt,
                }));
            }
        }
    }

    let evidence_dir = store.evidence_dir(&action_id)?;
    let media = stage_media(&media, &evidence_dir)?;
    page.navigate_with_timeout(CREATOR_IMAGE_URL, 60.0).await?;
    wait_for_creator(&page, Duration::from_secs(30)).await?;
    select_image_mode(&page).await?;
    wait_for_image_input(&page, Duration::from_secs(20)).await?;
    let paths = media
        .iter()
        .map(|item| item.upload_path.clone())
        .collect::<Vec<_>>();
    page.set_file_input_files(IMAGE_INPUT_SELECTOR, &paths)
        .await?;
    let file_signature = capture_upload_file_signature(&page).await?;
    let expected_file_signature = expected_file_signature(&media);
    if file_signature != expected_file_signature {
        anyhow::bail!("selected upload files do not match the staged media hashes");
    }
    let uploaded = wait_for_editor(&page, media.len(), Duration::from_secs(90)).await?;

    fill_field(&page, TITLE_SELECTOR, &title).await?;
    fill_field(&page, BODY_SELECTOR, &text).await?;
    let prepared = creator_state(&page).await?;
    let media_signature = prepared_media_signature(&prepared, media.len())?;
    verify_prepared_state(
        &prepared,
        &actor.id,
        &title,
        &text,
        media.len(),
        &file_signature,
        &media_signature,
    )?;
    let publish_control = page.flattened_exact_text_button("发布", "bg-red").await?;
    verify_publish_control(&publish_control)?;

    let prepared_shot = evidence_dir.join("prepared-before-submit.png");
    page.save_screenshot(&prepared_shot, false).await?;
    set_private_file(&prepared_shot)?;
    let preview = ActionPreview {
        text: Some(text.clone()),
        evidence: json!({
            "title": title,
            "text": text,
            "media": media.iter().map(MediaFile::evidence).collect::<Vec<_>>(),
            "file_signature": file_signature,
            "media_signature": media_signature,
            "transport": "daemon_persistent_browser_websocket",
            "submit_policy": "single_click_no_retry",
        }),
    };
    let receipt = if existing_draft {
        store.replace_draft_preview(&action_id, &actor.id, &target_id, preview)?
    } else {
        store.create_draft(
            &idempotency_key,
            "xhs",
            SocialActionKind::Publish,
            ActionTarget {
                id: target_id.clone(),
                url: CREATOR_URL.to_string(),
            },
            actor.clone(),
            preview,
        )?
    };
    let receipt = store.mark_prepared(receipt.action_id(), &actor.id, &target_id, 600)?;
    let result = json!({
        "ok": true,
        "status": "prepared",
        "action_id": receipt.action_id(),
        "actor": actor,
        "media_count": media.len(),
        "upload_state": uploaded,
        "publish_control": publish_control,
        "evidence": { "prepared_screenshot": path_string(&prepared_shot) },
        "next": format!("socai xhs commit-action --action-id {}", receipt.action_id()),
        "receipt": receipt,
    });
    write_evidence(&evidence_dir, "prepare.json", &result).await?;
    Ok(result)
}

pub async fn commit_action(page: Arc<PageSession>, action_id: String) -> anyhow::Result<Value> {
    let store = ActionStore::open_default();
    let receipt = store.load(action_id.trim())?;
    verify_publish_receipt(&receipt)?;
    match receipt.status() {
        SocialActionStatus::Committed | SocialActionStatus::Reconciled => {
            return Ok(json!({
                "ok": true,
                "status": receipt.status(),
                "action_id": receipt.action_id(),
                "idempotent_replay": true,
                "final_publish_clicked": false,
                "receipt": receipt,
            }));
        }
        SocialActionStatus::CommitUnknown | SocialActionStatus::Committing => {
            return Ok(json!({
                "ok": false,
                "status": "commit_unknown",
                "action_id": receipt.action_id(),
                "reason": "a submit attempt was already reserved; reconcile instead of retrying",
                "final_publish_clicked": false,
                "receipt": receipt,
            }));
        }
        SocialActionStatus::Draft => {
            anyhow::bail!("action is still a draft; run prepare-publish again");
        }
        SocialActionStatus::Prepared => {}
    }

    verify_staged_media(&receipt)?;
    let state = wait_for_creator(&page, Duration::from_secs(15)).await?;
    verify_prepared_receipt_state(&state, &receipt)?;
    let precommit_target_ids = collect_precommit_note_ids(&page, &receipt.actor().id).await?;
    let located_control = page.flattened_exact_text_button("发布", "bg-red").await?;
    verify_publish_control(&located_control)?;

    let evidence_dir = store.evidence_dir(receipt.action_id())?;
    let before_shot = evidence_dir.join("before-final-click.png");
    page.save_screenshot(&before_shot, false).await?;
    set_private_file(&before_shot)?;
    let immediate_state = creator_state(&page).await?;
    verify_prepared_receipt_state(&immediate_state, &receipt)?;
    let observed_actor = actor_from_state(&immediate_state)?;
    let observed_target = observed_target_id(&immediate_state)?;
    let publish_control = page.flattened_exact_text_button("发布", "bg-red").await?;
    verify_publish_control(&publish_control)?;
    if located_control.get("backend_node_id") != publish_control.get("backend_node_id") {
        anyhow::bail!("final publish control changed during commit validation");
    }
    let x = publish_control
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("publish control is missing x coordinate"))?;
    let y = publish_control
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("publish control is missing y coordinate"))?;
    let reserved = store.begin_commit(
        receipt.action_id(),
        &observed_actor.id,
        &observed_target,
        precommit_target_ids,
    )?;

    let attempt = async {
        page.click(x, y).await?;
        wait_for_publish_result(&page, Duration::from_secs(45)).await
    }
    .await;
    let (verified, observation, error) = match attempt {
        Ok(observation) => (true, Some(observation), None),
        Err(error) => (false, None, Some(format!("{error:#}"))),
    };
    let receipt = store.finish_commit(reserved.action_id(), verified)?;
    let after_shot = evidence_dir.join("after-final-click.png");
    let screenshot_error = page
        .save_screenshot(&after_shot, false)
        .await
        .err()
        .map(|error| format!("{error:#}"));
    if screenshot_error.is_none() {
        set_private_file(&after_shot)?;
    }
    let result = json!({
        "ok": verified,
        "status": if verified { "committed" } else { "commit_unknown" },
        "action_id": receipt.action_id(),
        "final_publish_clicked": true,
        "click_count": 1,
        "publish_control": publish_control,
        "observation": observation,
        "error": error,
        "screenshot_error": screenshot_error,
        "evidence": {
            "before_final_click": path_string(&before_shot),
            "after_final_click": path_string(&after_shot),
        },
        "next": format!("socai xhs reconcile-action --action-id {}", receipt.action_id()),
        "receipt": receipt,
    });
    write_evidence(&evidence_dir, "commit.json", &result).await?;
    Ok(result)
}

pub async fn reconcile_action(page: Arc<PageSession>, action_id: String) -> anyhow::Result<Value> {
    let store = ActionStore::open_default();
    let receipt = store.load(action_id.trim())?;
    verify_publish_receipt(&receipt)?;
    if receipt.status() == SocialActionStatus::Reconciled {
        return Ok(json!({
            "ok": true,
            "status": "reconciled",
            "action_id": receipt.action_id(),
            "idempotent_replay": true,
            "receipt": receipt,
        }));
    }
    if matches!(
        receipt.status(),
        SocialActionStatus::Draft | SocialActionStatus::Prepared
    ) {
        anyhow::bail!("action has no reserved submit attempt to reconcile");
    }

    page.navigate_with_timeout(NOTE_MANAGER_URL, 60.0).await?;
    let title = preview_string(&receipt, "title")?;
    let commit_started_at = receipt
        .commit_started_at()
        .ok_or_else(|| anyhow!("action receipt is missing its durable commit reservation time"))?;
    let precommit_target_ids = receipt
        .precommit_target_ids()
        .ok_or_else(|| anyhow!("action receipt is missing its complete precommit manager scan"))?;
    let observation = wait_for_manager_note(
        &page,
        &receipt.actor().id,
        &title,
        commit_started_at,
        precommit_target_ids,
        Duration::from_secs(45),
    )
    .await;
    let evidence_dir = store.evidence_dir(receipt.action_id())?;
    let manager_shot = evidence_dir.join("note-manager-reconcile.png");
    let screenshot_error = page
        .save_screenshot(&manager_shot, false)
        .await
        .err()
        .map(|error| format!("{error:#}"));
    if screenshot_error.is_none() {
        set_private_file(&manager_shot)?;
    }
    let observation = match observation {
        Ok(observation) => observation,
        Err(error) => {
            let result = json!({
                "ok": false,
                "status": "reconcile_unknown",
                "committed": Value::Null,
                "action_id": receipt.action_id(),
                "reason": format!("{error:#}"),
                "screenshot_error": screenshot_error,
                "evidence": { "note_manager": path_string(&manager_shot) },
                "receipt": receipt,
            });
            write_evidence(&evidence_dir, "reconcile.json", &result).await?;
            return Ok(result);
        }
    };
    let note_id = observation
        .get("note_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("positive reconciliation is missing a platform note ID"))?;
    let receipt = store.reconcile_committed(receipt.action_id(), note_id)?;
    let result = json!({
        "ok": true,
        "status": "reconciled",
        "committed": true,
        "action_id": receipt.action_id(),
        "observation": observation,
        "screenshot_error": screenshot_error,
        "evidence": { "note_manager": path_string(&manager_shot) },
        "receipt": receipt,
    });
    write_evidence(&evidence_dir, "reconcile.json", &result).await?;
    Ok(result)
}

#[derive(Clone)]
struct MediaFile {
    source_path: PathBuf,
    upload_path: PathBuf,
    sha256: String,
    bytes: u64,
}

impl MediaFile {
    fn evidence(&self) -> Value {
        json!({
            "source_path": path_string(&self.source_path),
            "staged_path": path_string(&self.upload_path),
            "sha256": self.sha256,
            "bytes": self.bytes,
        })
    }
}

fn inspect_media(raw: &[String]) -> anyhow::Result<Vec<MediaFile>> {
    if raw.is_empty() || raw.len() > MAX_IMAGES {
        anyhow::bail!("prepare-publish requires between 1 and {MAX_IMAGES} images");
    }
    raw.iter()
        .map(|raw| {
            let path = PathBuf::from(raw)
                .canonicalize()
                .with_context(|| format!("media file does not exist: {raw}"))?;
            if !path.is_file() {
                anyhow::bail!("media path is not a file: {}", path.display());
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                anyhow::bail!("unsupported image extension: .{extension}");
            }
            let bytes = path.metadata()?.len();
            if bytes == 0 || bytes > MAX_IMAGE_BYTES {
                anyhow::bail!(
                    "image size must be between 1 byte and {} MiB: {}",
                    MAX_IMAGE_BYTES / 1024 / 1024,
                    path.display()
                );
            }
            Ok(MediaFile {
                source_path: path.clone(),
                upload_path: path.clone(),
                sha256: hash_file(&path)?,
                bytes,
            })
        })
        .collect()
}

fn stage_media(media: &[MediaFile], evidence_dir: &Path) -> anyhow::Result<Vec<MediaFile>> {
    let directory = evidence_dir.join("media");
    ensure_private_directory(&directory)?;
    media
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let extension = item
                .source_path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("img")
                .to_ascii_lowercase();
            let destination = directory.join(format!("{index:02}.{extension}"));
            if destination.exists() {
                if hash_file(&destination)? != item.sha256 {
                    anyhow::bail!("staged media hash changed: {}", destination.display());
                }
            } else {
                let temporary = directory.join(format!(".{index:02}.{}.tmp", uuid::Uuid::new_v4()));
                let result = (|| -> anyhow::Result<()> {
                    let mut input = File::open(&item.source_path)?;
                    let mut output = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&temporary)?;
                    std::io::copy(&mut input, &mut output)?;
                    output.flush()?;
                    output.sync_all()?;
                    set_private_file(&temporary)?;
                    fs::rename(&temporary, &destination)?;
                    set_private_file(&destination)?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                result?;
            }
            Ok(MediaFile {
                source_path: item.source_path.clone(),
                upload_path: destination,
                sha256: item.sha256.clone(),
                bytes: item.bytes,
            })
        })
        .collect()
}

fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_copy(title: &str, text: &str) -> anyhow::Result<()> {
    let title_len = title.chars().count();
    let text_len = text.chars().count();
    if title_len == 0 || title_len > 20 {
        anyhow::bail!("title must contain 1 to 20 characters");
    }
    if text_len == 0 || text_len > 1000 {
        anyhow::bail!("text must contain 1 to 1000 characters");
    }
    Ok(())
}

fn publish_target_id(title: &str, text: &str, media: &[MediaFile]) -> String {
    publish_target_id_from_hashes(title, text, media.iter().map(|item| item.sha256.as_str()))
}

fn publish_target_id_from_hashes<'a>(
    title: &str,
    text: &str,
    hashes: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"socai-xhs-image-note-v1\0");
    hasher.update(title.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    for hash in hashes {
        hasher.update([0]);
        hasher.update(hash.as_bytes());
    }
    format!("xhs-note:{:x}", hasher.finalize())
}

fn observed_target_id(state: &Value) -> anyhow::Result<String> {
    let title = state
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("creator state is missing the title"))?;
    let text = state
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("creator state is missing the body"))?;
    let files = state
        .get("file_signature")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("creator state is missing upload-file identity"))?;
    let hashes = files
        .iter()
        .map(|item| {
            item.get("sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("upload-file identity is missing sha256"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(publish_target_id_from_hashes(title, text, hashes))
}

async fn ensure_creator_open(page: &PageSession) -> anyhow::Result<()> {
    let info = page.page_info().await.unwrap_or(Value::Null);
    let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
    if !url.starts_with("https://creator.xiaohongshu.com/") {
        page.navigate_with_timeout(CREATOR_URL, 60.0).await?;
    }
    Ok(())
}

async fn select_image_mode(page: &PageSession) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut latest = Value::Null;
    while Instant::now() < deadline {
        latest = page
            .evaluate_action(
                r#"
return (() => {
  const ready = document.querySelector("input[type='file'][accept*='.jpg']");
  if (ready) return {ok:true,status:'already_selected'};
  const visible = el => {
    if (!el || !el.getBoundingClientRect) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0 &&
      s.display !== 'none' && s.visibility !== 'hidden';
  };
  const candidates = [...document.querySelectorAll('.creator-tab,[role="tab"],button,div,span')]
    .filter(el => visible(el) && (el.innerText || el.textContent || '').trim() === '上传图文')
    .sort((a,b) => a.children.length - b.children.length);
  const tab = candidates[0];
  if (!tab) {
    return {
      ok:false,
      status:'image_tab_not_found',
      url:location.href,
      body:(document.body && document.body.innerText || '').replace(/\s+/g,' ').trim().slice(0,1000),
    };
  }
  tab.click();
  return {ok:true,status:'image_tab_selected',tag:tab.tagName.toLowerCase(),class_name:String(tab.className || '')};
})();
"#,
            )
            .await?;
        if latest.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("could not select Xiaohongshu image-note mode; last state: {latest}")
}

async fn fill_field(page: &PageSession, selector: &str, text: &str) -> anyhow::Result<()> {
    page.focus_and_select(selector).await?;
    page.type_text(text).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    Ok(())
}

async fn capture_upload_file_signature(page: &PageSession) -> anyhow::Result<Value> {
    let signature = page
        .evaluate_json(
            r#"
return (async () => {
  const input = document.querySelector("input[type='file'][accept*='.jpg']");
  const files = [...(input && input.files || [])];
  const bytesToHex = bytes => [...new Uint8Array(bytes)]
    .map(value => value.toString(16).padStart(2, '0')).join('');
  const signature = [];
  for (const file of files) {
    signature.push({
      name: file.name,
      bytes: file.size,
      mime: file.type,
      sha256: bytesToHex(await crypto.subtle.digest('SHA-256', await file.arrayBuffer())),
    });
  }
  if (signature.length) {
    Object.defineProperty(globalThis, '__socaiXhsUploadBinding', {
      value: Object.freeze(signature.map(item => Object.freeze({...item}))),
      writable: false,
      configurable: false,
    });
  }
  return signature;
})();
"#,
        )
        .await?;
    if signature.as_array().is_none_or(Vec::is_empty) {
        anyhow::bail!("CDP file input did not retain the selected upload long enough to verify it");
    }
    Ok(signature)
}

async fn creator_state(page: &PageSession) -> anyhow::Result<Value> {
    let mut state = page
        .evaluate_json(
            r#"
return (async () => {
  const body = (document.body && document.body.innerText || '').replace(/\s+/g, ' ').trim();
  const visible = el => {
    if (!el || !el.getBoundingClientRect) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden';
  };
  const title = [...document.querySelectorAll('input,textarea')].find(el =>
    (el.placeholder || '').includes('标题'));
  const editor = [...document.querySelectorAll('[contenteditable="true"]')].find(el =>
    el.classList.contains('ProseMirror'));
  const actorMatch = body.match(/^创作服务平台\s+(.+?)\s+发布笔记(?:\s|$)/);
  const avatar = [...document.querySelectorAll('img')].find(el =>
    el.classList.contains('user_avatar'));
  const bytesToHex = bytes => [...new Uint8Array(bytes)]
    .map(value => value.toString(16).padStart(2, '0')).join('');
  const fileInput = document.querySelector("input[type='file'][accept*='.jpg']");
  const fileSignature = [];
  for (const file of [...(fileInput && fileInput.files || [])]) {
    fileSignature.push({
      name: file.name,
      bytes: file.size,
      mime: file.type,
      sha256: bytesToHex(await crypto.subtle.digest('SHA-256', await file.arrayBuffer())),
    });
  }
  const boundFileSignature = globalThis.__socaiXhsUploadBinding || fileSignature;
  const mediaMatch = body.match(/图片编辑\s+(\d+)\s*\/\s*18/);
  const mediaSignature = [...document.querySelectorAll('img')]
    .filter(el => {
      if (!visible(el) || el.classList.contains('user_avatar')) return false;
      const r = el.getBoundingClientRect();
      return r.top >= 70 && r.top < 450 && r.width >= 48 && r.height >= 48;
    })
    .map(el => {
      const r = el.getBoundingClientRect();
      return {
        src: el.currentSrc || el.src || '',
        x: Math.round(r.left),
        y: Math.round(r.top),
        width: Math.round(r.width),
        height: Math.round(r.height),
      };
    })
    .sort((a,b) => a.y - b.y || a.x - b.x);
  return {
    url: location.href,
    body_excerpt: body.slice(0, 8000),
    actor: actorMatch ? actorMatch[1].trim() : '',
    actor_avatar: avatar ? (() => {
      try {
        const url = new URL(avatar.currentSrc || avatar.src || '');
        return url.origin + url.pathname;
      } catch (_) { return avatar.currentSrc || avatar.src || ''; }
    })() : '',
    title: title ? title.value : '',
    text: editor ? (editor.innerText || editor.textContent || '').trim() : '',
    editor_ready: !!title && !!editor,
    image_input_ready: !!document.querySelector("input[type='file'][accept*='.jpg']"),
    media_count: mediaMatch ? Number(mediaMatch[1]) : 0,
    file_signature: boundFileSignature,
    media_signature: mediaSignature,
    published: location.pathname === '/publish/success' || location.search.includes('published=true'),
    login_required: /登录|扫码登录/.test(body) && !actorMatch,
  };
})();
"#,
        )
        .await?;
    state["browser_context_id"] = Value::String(page.browser_context_id().await?);
    Ok(state)
}

async fn wait_for_creator(page: &PageSession, timeout: Duration) -> anyhow::Result<Value> {
    wait_for_state(
        page,
        timeout,
        |state| {
            !state
                .get("actor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty()
        },
        "creator account did not become ready",
    )
    .await
}

async fn wait_for_image_input(page: &PageSession, timeout: Duration) -> anyhow::Result<Value> {
    wait_for_state(
        page,
        timeout,
        |state| state.get("image_input_ready").and_then(Value::as_bool) == Some(true),
        "creator image input did not become ready",
    )
    .await
}

async fn wait_for_editor(
    page: &PageSession,
    media_count: usize,
    timeout: Duration,
) -> anyhow::Result<Value> {
    wait_for_state(
        page,
        timeout,
        |state| {
            state.get("editor_ready").and_then(Value::as_bool) == Some(true)
                && state.get("media_count").and_then(Value::as_u64) == Some(media_count as u64)
        },
        "uploaded media did not reach the creator editor",
    )
    .await
}

async fn wait_for_publish_result(page: &PageSession, timeout: Duration) -> anyhow::Result<Value> {
    wait_for_state(
        page,
        timeout,
        |state| state.get("published").and_then(Value::as_bool) == Some(true),
        "publish result was not observed after the single click",
    )
    .await
}

fn verify_publish_control(control: &Value) -> anyhow::Result<()> {
    if control.get("ok").and_then(Value::as_bool) != Some(true)
        || control.get("disabled").and_then(Value::as_bool) == Some(true)
        || control.get("hit_owned").and_then(Value::as_bool) != Some(true)
    {
        anyhow::bail!("final publish control is not enabled or is obscured");
    }
    if control
        .get("backend_node_id")
        .and_then(Value::as_i64)
        .is_none()
        || control.get("x").and_then(Value::as_f64).is_none()
        || control.get("y").and_then(Value::as_f64).is_none()
    {
        anyhow::bail!("final publish control is missing stable CDP identity or coordinates");
    }
    Ok(())
}

async fn wait_for_state(
    page: &PageSession,
    timeout: Duration,
    accept: impl Fn(&Value) -> bool,
    timeout_message: &str,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut latest = Value::Null;
    while Instant::now() < deadline {
        latest = creator_state(page).await.unwrap_or(Value::Null);
        if latest.get("login_required").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!("Xiaohongshu creator account is logged out");
        }
        if accept(&latest) {
            return Ok(latest);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow!("{timeout_message}; last state: {latest}"))
}

fn actor_from_state(state: &Value) -> anyhow::Result<ActionActor> {
    let display_name = state
        .get("actor")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("could not identify the logged-in creator account"))?;
    let browser_context = state
        .get("browser_context_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("could not identify the Chrome profile context"))?;
    let mut hasher = Sha256::new();
    let avatar = state
        .get("actor_avatar")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("could not identify the visible creator avatar"))?;
    hasher.update(b"socai-xhs-browser-ui-account-v1\0");
    hasher.update(browser_context.as_bytes());
    hasher.update([0]);
    hasher.update(display_name.as_bytes());
    hasher.update([0]);
    hasher.update(avatar.as_bytes());
    Ok(ActionActor {
        id: format!("xhs-creator:{:x}", hasher.finalize()),
        display_name: display_name.to_string(),
    })
}

fn verify_prepared_state(
    state: &Value,
    actor_id: &str,
    title: &str,
    text: &str,
    media_count: usize,
    file_signature: &Value,
    media_signature: &Value,
) -> anyhow::Result<()> {
    let actor = actor_from_state(state)?;
    if actor.id != actor_id {
        anyhow::bail!("creator account changed after prepare; refusing to continue");
    }
    if state.get("title").and_then(Value::as_str) != Some(title) {
        anyhow::bail!("visible title does not match the prepared receipt");
    }
    if state.get("text").and_then(Value::as_str) != Some(text) {
        anyhow::bail!("visible body does not match the prepared receipt");
    }
    if state.get("media_count").and_then(Value::as_u64) != Some(media_count as u64) {
        anyhow::bail!("visible media count does not match the prepared receipt");
    }
    if state.get("file_signature") != Some(file_signature) {
        anyhow::bail!("selected upload files do not match the staged media hashes");
    }
    if state.get("media_signature") != Some(media_signature) {
        anyhow::bail!("visible media previews changed after prepare");
    }
    Ok(())
}

fn prepared_media_signature(state: &Value, media_count: usize) -> anyhow::Result<Value> {
    let signature = state
        .get("media_signature")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("creator editor did not expose media preview identity"))?;
    if signature.len() < media_count {
        anyhow::bail!(
            "creator media preview identity is incomplete: expected at least {media_count}, found {}",
            signature.len()
        );
    }
    Ok(Value::Array(signature.clone()))
}

fn expected_file_signature(media: &[MediaFile]) -> Value {
    Value::Array(
        media
            .iter()
            .map(|item| {
                json!({
                    "name": item.upload_path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
                    "bytes": item.bytes,
                    "mime": image_mime(&item.upload_path),
                    "sha256": item.sha256,
                })
            })
            .collect(),
    )
}

fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "",
    }
}

fn verify_prepared_receipt_state(state: &Value, receipt: &ActionReceipt) -> anyhow::Result<()> {
    let title = preview_string(receipt, "title")?;
    let text = preview_string(receipt, "text")?;
    let media_count = receipt
        .preview()
        .evidence
        .get("media")
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| anyhow!("publish receipt is missing media evidence"))?;
    let signature = receipt
        .preview()
        .evidence
        .get("media_signature")
        .ok_or_else(|| anyhow!("publish receipt is missing media preview identity"))?;
    let file_signature = receipt
        .preview()
        .evidence
        .get("file_signature")
        .ok_or_else(|| anyhow!("publish receipt is missing upload-file identity"))?;
    verify_prepared_state(
        state,
        &receipt.actor().id,
        &title,
        &text,
        media_count,
        file_signature,
        signature,
    )
}

fn verify_publish_receipt(receipt: &ActionReceipt) -> anyhow::Result<()> {
    if receipt.platform() != "xhs" || receipt.action() != SocialActionKind::Publish {
        anyhow::bail!("action receipt is not an XHS publish action");
    }
    Ok(())
}

fn verify_requested_intent(
    receipt: &ActionReceipt,
    actor: &ActionActor,
    target_id: &str,
    title: &str,
    text: &str,
    media: &[MediaFile],
) -> anyhow::Result<()> {
    verify_publish_receipt(receipt)?;
    if receipt.actor().id != actor.id || receipt.target().id != target_id {
        anyhow::bail!("existing action belongs to a different actor or publish target");
    }
    if preview_string(receipt, "title")? != title || preview_string(receipt, "text")? != text {
        anyhow::bail!("existing action content does not match the requested publish intent");
    }
    let stored = receipt
        .preview()
        .evidence
        .get("media")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("existing action is missing media evidence"))?;
    let hashes = stored
        .iter()
        .map(|item| {
            item.get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let requested = media
        .iter()
        .map(|item| item.sha256.as_str())
        .collect::<Vec<_>>();
    if hashes != requested {
        anyhow::bail!("existing action media hashes do not match the requested images");
    }
    Ok(())
}

fn verify_staged_media(receipt: &ActionReceipt) -> anyhow::Result<()> {
    let store = ActionStore::open_default();
    let evidence_root = store.evidence_dir(receipt.action_id())?.canonicalize()?;
    let media = receipt
        .preview()
        .evidence
        .get("media")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("publish receipt is missing media evidence"))?;
    if media.is_empty() {
        anyhow::bail!("publish receipt contains no staged media");
    }
    for item in media {
        let raw_path = item
            .get("staged_path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("publish receipt is missing staged media path"))?;
        let expected_hash = item
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("publish receipt is missing staged media hash"))?;
        let expected_bytes = item
            .get("bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("publish receipt is missing staged media size"))?;
        let path = PathBuf::from(raw_path).canonicalize()?;
        if !path.starts_with(&evidence_root) {
            anyhow::bail!("staged media escaped its action evidence directory");
        }
        if path.metadata()?.len() != expected_bytes || hash_file(&path)? != expected_hash {
            anyhow::bail!("staged media changed after prepare: {}", path.display());
        }
    }
    Ok(())
}

fn preview_string(receipt: &ActionReceipt, key: &str) -> anyhow::Result<String> {
    receipt
        .preview()
        .evidence
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("publish receipt is missing {key}"))
}

async fn collect_precommit_note_ids(
    page: &PageSession,
    actor_id: &str,
) -> anyhow::Result<Vec<String>> {
    let manager = page.create_background_sibling(NOTE_MANAGER_URL).await?;
    let result = collect_complete_manager_ids(&manager, actor_id, Duration::from_secs(45)).await;
    let close_result = manager.close().await;
    match (result, close_result) {
        (Ok(ids), Ok(())) => Ok(ids),
        (Ok(_), Err(error)) => Err(error).context("failed to close precommit note-manager tab"),
        (Err(error), _) => Err(error),
    }
}

async fn collect_complete_manager_ids(
    page: &PageSession,
    actor_id: &str,
    timeout: Duration,
) -> anyhow::Result<Vec<String>> {
    wait_for_creator(page, Duration::from_secs(30)).await?;
    let deadline = Instant::now() + timeout;
    let mut seen = BTreeSet::new();
    let mut latest = Value::Null;
    while Instant::now() < deadline {
        let actor_state = creator_state(page).await?;
        let observed_actor = match actor_from_state(&actor_state) {
            Ok(actor) => actor,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if observed_actor.id != actor_id {
            anyhow::bail!("creator account changed during the precommit manager scan");
        }
        latest = page
            .evaluate_action(
                r#"
return (async () => {
  const body = (document.body && document.body.innerText || '').replace(/\s+/g, ' ').trim();
  const visible = el => {
    if (!el || !el.getBoundingClientRect) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden';
  };
  const noteId = card => {
    for (const node of [card, ...card.querySelectorAll('*')]) {
      const impression = node.getAttribute && node.getAttribute('data-impression');
      if (impression) {
        try {
          const value = JSON.parse(impression)?.noteTarget?.value?.noteId || '';
          if (/^[0-9a-f]{24}$/i.test(value)) return value.toLowerCase();
        } catch (_) {}
      }
      for (const name of ['data-note-id','data-noteid','data-item-id','data-target-id']) {
        const value = node.getAttribute && node.getAttribute(name);
        if (/^[0-9a-f]{24}$/i.test(value || '')) return value.toLowerCase();
      }
      const match = (node.href || '').match(/\/(?:explore|discovery\/item)\/([0-9a-f]{24})(?:[/?#]|$)/i);
      if (match) return match[1].toLowerCase();
    }
    return '';
  };
  const cardNodes = [...document.querySelectorAll('.note-card')];
  const noteIds = cardNodes.map(noteId);
  const allMatch = body.match(/(?:^|\s)全部\s*(\d+)(?:\s|$)/);
  let scrollRoot = document.scrollingElement;
  if (cardNodes.length) {
    for (let node = cardNodes[0].parentElement; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.scrollHeight > node.clientHeight + 8 && /(auto|scroll)/.test(style.overflowY)) {
        scrollRoot = node;
        break;
      }
    }
  }
  const before = scrollRoot ? scrollRoot.scrollTop : 0;
  const max = scrollRoot ? Math.max(0, scrollRoot.scrollHeight - scrollRoot.clientHeight) : 0;
  const atEnd = !scrollRoot || before >= max - 4;
  if (scrollRoot) {
    await socaiAction(() => { scrollRoot.scrollTop = atEnd ? 0 : Math.min(max, before + Math.max(240, scrollRoot.clientHeight * 0.8)); });
  }
  const next = [...document.querySelectorAll('button,[role="button"]')].find(el => {
    const text = (el.innerText || el.textContent || '').trim();
    const label = el.getAttribute('aria-label') || '';
    return visible(el) && (text === '下一页' || /next|下一页/i.test(label));
  });
  let nextClicked = false;
  if (atEnd && next && !next.disabled && next.getAttribute('aria-disabled') !== 'true') {
    await socaiAction(() => next.click());
    nextClicked = true;
  }
  return {
    note_ids: noteIds,
    total_count: allMatch ? Number(allMatch[1]) : null,
    at_end: atEnd,
    next_clicked: nextClicked,
    body_excerpt: body.slice(0, 8000),
  };
})();
"#,
            )
            .await?;
        let current_ids = latest
            .get("note_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("note manager did not expose card identifiers"))?;
        for note_id in current_ids.iter().filter_map(Value::as_str) {
            if !note_id.is_empty() {
                seen.insert(note_id.to_string());
            }
        }
        let total_count = latest.get("total_count").and_then(Value::as_u64);
        let complete = total_count
            .map(|total| seen.len() as u64 >= total)
            .unwrap_or_else(|| {
                latest.get("at_end").and_then(Value::as_bool) == Some(true)
                    && latest.get("next_clicked").and_then(Value::as_bool) != Some(true)
                    && current_ids
                        .iter()
                        .all(|value| value.as_str().is_some_and(|id| !id.is_empty()))
            });
        if complete {
            return Ok(seen.into_iter().collect());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow!(
        "precommit manager scan did not cover the complete result set; scanned {} IDs; last state: {latest}",
        seen.len()
    ))
}

async fn wait_for_manager_note(
    page: &PageSession,
    actor_id: &str,
    title: &str,
    commit_started_at: DateTime<Utc>,
    precommit_target_ids: &[String],
    timeout: Duration,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut latest = Value::Null;
    let mut seen = BTreeMap::<String, Value>::new();
    let precommit_target_ids = precommit_target_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let china = FixedOffset::east_opt(8 * 60 * 60).expect("valid China offset");
    let earliest = commit_started_at - chrono::Duration::seconds(90);
    while Instant::now() < deadline {
        let actor_state = creator_state(page).await?;
        let observed_actor = match actor_from_state(&actor_state) {
            Ok(actor) => actor,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if observed_actor.id != actor_id {
            anyhow::bail!("creator account changed before reconciliation");
        }
        latest = page
            .evaluate_action(
                r#"
return (async () => {
  const body = (document.body && document.body.innerText || '').replace(/\s+/g, ' ').trim();
  const visible = el => {
    if (!el || !el.getBoundingClientRect) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden';
  };
  const noteId = card => {
    const nodes = [card, ...card.querySelectorAll('*')];
    for (const node of nodes) {
      const impression = node.getAttribute && node.getAttribute('data-impression');
      if (impression) {
        try {
          const value = JSON.parse(impression)?.noteTarget?.value?.noteId || '';
          if (/^[0-9a-f]{24}$/i.test(value)) return value.toLowerCase();
        } catch (_) {}
      }
      for (const name of ['data-note-id','data-noteid','data-item-id','data-target-id']) {
        const value = node.getAttribute && node.getAttribute(name);
        if (/^[0-9a-f]{24}$/i.test(value || '')) return value.toLowerCase();
      }
      const href = node.href || '';
      const match = href.match(/\/(?:explore|discovery\/item)\/([0-9a-f]{24})(?:[/?#]|$)/i);
      if (match) return match[1].toLowerCase();
    }
    return '';
  };
  const structuralTime = (card, titleEl) => {
    const exact = /^\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}$/;
    return [...card.querySelectorAll('time,[datetime],*')]
      .filter(el => !titleEl || (el !== titleEl && !titleEl.contains(el)))
      .map(el => ({el, text:(el.getAttribute('datetime') || el.textContent || '').trim()}))
      .find(item => item.el.children.length === 0 && exact.test(item.text))?.text || '';
  };
  const cardNodes = [...document.querySelectorAll('.note-card')];
  const cards = cardNodes.map(card => {
    const titleEl = card.querySelector('.note-card__title');
    const cover = card.querySelector('img');
    return {
      note_id: noteId(card),
      title: (titleEl && (titleEl.innerText || titleEl.textContent) || '').trim(),
      published_at: structuralTime(card, titleEl),
      cover_src: cover ? (cover.currentSrc || cover.src || '') : '',
    };
  });
  const allMatch = body.match(/(?:^|\s)全部\s*(\d+)(?:\s|$)/);
  let scrollRoot = document.scrollingElement;
  if (cardNodes.length) {
    for (let node = cardNodes[0].parentElement; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.scrollHeight > node.clientHeight + 8 && /(auto|scroll)/.test(style.overflowY)) {
        scrollRoot = node;
        break;
      }
    }
  }
  const before = scrollRoot ? scrollRoot.scrollTop : 0;
  const max = scrollRoot ? Math.max(0, scrollRoot.scrollHeight - scrollRoot.clientHeight) : 0;
  const atEnd = !scrollRoot || before >= max - 4;
  if (scrollRoot) {
    await socaiAction(() => { scrollRoot.scrollTop = atEnd ? 0 : Math.min(max, before + Math.max(240, scrollRoot.clientHeight * 0.8)); });
  }
  const next = [...document.querySelectorAll('button,[role="button"]')].find(el => {
    const text = (el.innerText || el.textContent || '').trim();
    const label = el.getAttribute('aria-label') || '';
    return visible(el) && (text === '下一页' || /next|下一页/i.test(label));
  });
  let nextClicked = false;
  if (atEnd && next && !next.disabled && next.getAttribute('aria-disabled') !== 'true') {
    await socaiAction(() => next.click());
    nextClicked = true;
  }
  return {
    url: location.href,
    cards,
    loaded_card_count: cards.length,
    total_count: allMatch ? Number(allMatch[1]) : null,
    at_end: atEnd,
    next_clicked: nextClicked,
    body_excerpt: body.slice(0, 8000),
  };
})();
"#,
            )
            .await?;
        let cards = latest
            .get("cards")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("note manager did not expose structured cards"))?;
        for card in cards {
            if let Some(note_id) = card
                .get("note_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                seen.insert(note_id.to_string(), card.clone());
            }
        }
        let total_count = latest.get("total_count").and_then(Value::as_u64);
        let full_result_set_seen = total_count
            .map(|total| seen.len() as u64 >= total)
            .unwrap_or_else(|| {
                latest.get("at_end").and_then(Value::as_bool) == Some(true)
                    && latest.get("next_clicked").and_then(Value::as_bool) != Some(true)
                    && cards.iter().all(|card| {
                        card.get("note_id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| !id.is_empty())
                    })
            });
        let now = Utc::now() + chrono::Duration::minutes(5);
        let matches = seen
            .values()
            .filter_map(|card| {
                let note_id = card.get("note_id").and_then(Value::as_str)?;
                if precommit_target_ids.contains(note_id) {
                    return None;
                }
                let card_title = card.get("title").and_then(Value::as_str)?;
                let raw_time = card.get("published_at").and_then(Value::as_str)?;
                let published_at = parse_manager_time(raw_time, china).ok()?;
                (card_title == title && published_at >= earliest && published_at <= now)
                    .then_some((card, published_at))
            })
            .collect::<Vec<_>>();
        if full_result_set_seen && matches.len() == 1 {
            let (card, published_at) = matches[0];
            return Ok(json!({
                "url": latest.get("url"),
                "note_id": card.get("note_id"),
                "title": card.get("title"),
                "published_at": published_at.to_rfc3339(),
                "cover_src": card.get("cover_src"),
                "scanned_unique_note_ids": seen.len(),
                "reported_total_count": total_count,
                "commit_started_at": commit_started_at.to_rfc3339(),
            }));
        }
        if matches.len() > 1 {
            anyhow::bail!(
                "multiple post-reservation note-manager cards match the receipt; reconciliation is ambiguous"
            );
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
    Err(anyhow!(
        "a unique post-reservation note ID was not found across the complete note-manager result set; scanned {} IDs; last state: {latest}",
        seen.len()
    ))
}

fn parse_manager_time(raw: &str, china: FixedOffset) -> anyhow::Result<DateTime<Utc>> {
    let value = NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M")?;
    let local = china
        .from_local_datetime(&value)
        .single()
        .ok_or_else(|| anyhow!("ambiguous note-manager timestamp"))?;
    Ok(local.with_timezone(&Utc))
}

async fn write_evidence(directory: &Path, name: &str, value: &Value) -> anyhow::Result<()> {
    ensure_private_directory(directory)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    let path = directory.join(name);
    tokio::fs::write(&path, bytes).await?;
    set_private_file(&path)?;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
