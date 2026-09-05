use serde_json::{json, Value};

use crate::cdp::connection::Cdp;
use crate::cdp::session::PageSession;

/// Thin page factory over one remote-debugging endpoint. Higher-level runtime
/// code decides whether a page belongs to a tool session, an agent run, or a
/// debug command.
pub struct PageSessionManager {
    cdp: Cdp,
}

struct PendingTarget {
    target_id: String,
    client: crate::cdp::raw_client::RawCdpClient,
    owner: Cdp,
    armed: bool,
}

impl PendingTarget {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingTarget {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let target_id = self.target_id.clone();
        let client = self.client.clone();
        let owner = self.owner.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            match client
                .execute("Target.closeTarget", json!({ "targetId": &target_id }))
                .await
            {
                Ok(_) => owner.unregister_owned_target(&target_id).await,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        target_id,
                        "failed to close partially created page target"
                    );
                }
            }
        });
    }
}

impl PageSessionManager {
    pub fn new(cdp: Cdp) -> Self {
        Self { cdp }
    }

    /// Open a new socai-owned tab and control only that target. All target
    /// lifecycle is routed through the browser websocket (`Target.*`) so
    /// existing and managed Chrome share one code path. This deliberately
    /// avoids browser-wide CDP target discovery/auto-attach, so unrelated user
    /// tabs are not instrumented.
    pub async fn create_page(&self, start_url: &str) -> anyhow::Result<PageSession> {
        self.create_page_with_options(start_url, false).await
    }

    /// Create an owned tab without bringing it to the foreground. This is used
    /// for short-lived control contexts that must not steal focus from the site
    /// tab the user is watching.
    pub async fn create_background_page(&self, start_url: &str) -> anyhow::Result<PageSession> {
        self.create_page_with_options(start_url, true).await
    }

    async fn create_page_with_options(
        &self,
        start_url: &str,
        background: bool,
    ) -> anyhow::Result<PageSession> {
        // Client and browser mode come from one locked read: the page is
        // labelled with the browser it is actually created in, even if the
        // connection is replaced while the target commands below are in flight.
        let (browser_client, remote_browser) = self
            .cdp
            .browser_client_with_mode()
            .await
            .ok_or_else(|| anyhow::anyhow!("CDP browser websocket is not connected"))?;
        self.create_page_via_browser_ws(browser_client, remote_browser, start_url, background)
            .await
    }

    async fn create_page_via_browser_ws(
        &self,
        browser_client: crate::cdp::raw_client::RawCdpClient,
        remote_browser: bool,
        start_url: &str,
        background: bool,
    ) -> anyhow::Result<PageSession> {
        let mut create_params = json!({ "url": blank_or_start_url(start_url) });
        if background {
            create_params["background"] = Value::Bool(true);
        }
        let created = browser_client
            .execute("Target.createTarget", create_params)
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Target.createTarget missing targetId"))?
            .to_string();
        let mut pending = PendingTarget {
            target_id: target_id.clone(),
            client: browser_client.clone(),
            owner: self.cdp.clone(),
            armed: true,
        };
        self.cdp.register_owned_target(target_id.clone()).await;

        let attached = browser_client
            .execute(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Target.attachToTarget missing sessionId"))?
            .to_string();

        pending.disarm();
        Ok(PageSession::attached(
            target_id,
            browser_client,
            session_id,
            self.cdp.clone(),
            remote_browser,
            background,
        ))
    }

    /// Close a page target by target id. This is stronger than consuming a
    /// `PageSession`: cancellation paths may only have a task snapshot and an
    /// id, or the page may still be held by tool `Arc`s.
    pub async fn close_target(&self, target_id: &str) -> anyhow::Result<bool> {
        let target_id = target_id.trim();
        if target_id.is_empty() {
            return Ok(false);
        }
        let browser_client = self
            .cdp
            .browser_client()
            .await
            .ok_or_else(|| anyhow::anyhow!("CDP browser websocket is not connected"))?;
        browser_client
            .execute("Target.closeTarget", json!({ "targetId": target_id }))
            .await?;
        self.cdp.unregister_owned_target(target_id).await;
        Ok(true)
    }
}

fn blank_or_start_url(start_url: &str) -> &str {
    let start_url = start_url.trim();
    if start_url.is_empty() {
        "about:blank"
    } else {
        start_url
    }
}
