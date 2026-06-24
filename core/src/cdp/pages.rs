use serde_json::{json, Value};

use crate::cdp::connection::Cdp;
use crate::cdp::endpoint;
use crate::cdp::session::PageSession;

/// Thin page factory over one remote-debugging endpoint. Higher-level runtime
/// code decides whether a page belongs to a tool session, an agent run, or a
/// debug command.
pub struct PageSessionManager {
    cdp: Cdp,
}

impl PageSessionManager {
    pub fn new(cdp: Cdp) -> Self {
        Self { cdp }
    }

    /// Open a new socai-owned tab and control only that target. This
    /// deliberately avoids browser-wide CDP target discovery/auto-attach, so
    /// unrelated user tabs are not instrumented.
    pub async fn create_page(&self, start_url: &str) -> anyhow::Result<PageSession> {
        if let Some(browser_client) = self.cdp.browser_client().await {
            return self
                .create_page_via_browser_ws(browser_client, start_url)
                .await;
        }

        let endpoint = self.cdp.endpoint().await?;
        let target = endpoint::create_debug_page(&endpoint, start_url).await?;
        let target_id = target.target_id.clone();
        let Some(ws) = target.web_socket_debugger_url else {
            let _ = endpoint::close_debug_target(&endpoint, &target_id).await;
            anyhow::bail!("created target missing webSocketDebuggerUrl: {target_id}");
        };
        let page = match PageSession::connect(target_id.clone(), &ws, self.cdp.clone()).await {
            Ok(page) => page,
            Err(err) => {
                let _ = endpoint::close_debug_target(&endpoint, &target_id).await;
                return Err(err);
            }
        };
        self.cdp.register_owned_target(target_id).await;
        Ok(page)
    }

    async fn create_page_via_browser_ws(
        &self,
        browser_client: crate::cdp::raw_client::RawCdpClient,
        start_url: &str,
    ) -> anyhow::Result<PageSession> {
        let created = browser_client
            .execute(
                "Target.createTarget",
                json!({ "url": blank_or_start_url(start_url) }),
            )
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Target.createTarget missing targetId"))?
            .to_string();

        let attached = match browser_client
            .execute(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await
        {
            Ok(attached) => attached,
            Err(err) => {
                let _ = browser_client
                    .execute("Target.closeTarget", json!({ "targetId": target_id }))
                    .await;
                return Err(err);
            }
        };
        let session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Target.attachToTarget missing sessionId"))?
            .to_string();

        self.cdp.register_owned_target(target_id.clone()).await;
        Ok(PageSession::attached(
            target_id,
            browser_client,
            session_id,
            self.cdp.clone(),
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
        if let Some(browser_client) = self.cdp.browser_client().await {
            browser_client
                .execute("Target.closeTarget", json!({ "targetId": target_id }))
                .await?;
            self.cdp.unregister_owned_target(target_id).await;
            return Ok(true);
        }
        let endpoint = self.cdp.endpoint().await?;
        let result = endpoint::close_debug_target(&endpoint, target_id).await;
        self.cdp.unregister_owned_target(target_id).await;
        result
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
