# Google login development and setup

The desktop account menu offers phone verification and **Sign in with Google**.
A signed-in phone account can **Link Google account**, preserving its user ID,
wallet and subscription. A Google-only account needs no phone number. Linking
rejects a Google identity already belonging to another account; there is no
automatic account or balance merge by matching email or device.

## Google Cloud setup

1. Open [Google Auth Platform](https://console.cloud.google.com/auth/overview)
   in the Google Cloud project for socai.
2. Configure **Branding** with the app name `socai`, support contact and required
   homepage/privacy links. Choose the appropriate **Audience**; use External
   when people outside your organization need to sign in.
3. Review the audience and publishing settings for the requested scopes. Configure
   test users and complete brand verification where required by Google before
   public rollout.
4. Under **Clients**, create an OAuth client of type **Desktop app**. A Web
   application client will not work with this implementation's dynamic local
   callback. Do not enable Gmail access: the requested scopes are only
   `openid email`.
5. Set the private backend's environment variables through its normal secret
   configuration mechanism:

   ```dotenv
   GOOGLE_CLIENT_ID=<desktop-client-id>.apps.googleusercontent.com
   GOOGLE_CLIENT_SECRET=<desktop-client-secret>
   ```

   Do not put the secret in frontend code, the public repository or build-time
   desktop environment variables. The desktop client secret is not a proof of
   app identity; PKCE and verified Google ID tokens protect this flow.
6. Deploy the companion `socai-server` implementation and restart it with these
   variables. The service must reach `https://oauth2.googleapis.com/token` and
   Google's signing-certificate endpoint. Test this from the deployment network,
   especially when the backend is hosted in mainland China.
7. Verify `GET /v1/auth/google/config` returns a public `client_id`. With missing
   configuration it returns 503, and the desktop shows a localized message to
   use phone sign-in. An older server returning 404 gets the same message.
8. Build/run the desktop app normally. For an isolated backend, set
   `SOCAI_PRO_BASE_URL=http://127.0.0.1:8000` in the desktop process; use HTTPS
   for deployed services.

Official references: [desktop OAuth and PKCE](https://developers.google.com/identity/protocols/oauth2/native-app)
and [server-side ID token verification](https://developers.google.com/identity/gsi/web/guides/verify-google-id-token).

## Backend without Google connectivity

The companion server supports a separate Google verifier. It can run under a
different cloud account; no shared VPC or database access is required. Keep user,
wallet, subscription and device-token issuance on the main backend. Forward only
the authorization code, PKCE verifier, loopback URI and nonce to the verifier.

On the main backend set `GOOGLE_CLIENT_ID`, `GOOGLE_RELAY_URL` and
`GOOGLE_RELAY_TOKEN`. The Google client secret is needed only on the verifier.
The deployed topology uses an automatically reconnecting, host-key-pinned SSH
tunnel: main-host `127.0.0.1:18787` to verifier-host `127.0.0.1:8787`. The relay
also requires a service token and exposes no public HTTP port. Deployment and
rollback instructions live in the companion server's `deploy/README-google-relay.md`.

Local operator SSH aliases and private-key locations are maintained outside the
repository in `~/.config/socai/cloud-inventory.md`. Do not commit credentials or
operator keys. Users still need browser access to Google themselves.

## Login does not open a browser

The app loads `/v1/auth/google/config` before opening the browser. An older
backend returning 404, or one returning 503 because credentials are missing,
cannot begin login. Deploy/configure the API before testing the button.
The UI shows an opening status first, switches to the browser instruction only
after the OS browser launch succeeds, and keeps failures visible in the account
popover. Restart/rebuild the desktop Rust shell if frontend and command versions
are out of sync.

## Flow and API contract

The Rust core binds an ephemeral `127.0.0.1` port, creates independent random
state, nonce and PKCE verifier values, then opens the system browser. Google
returns an authorization code to `/oauth/google/callback` on that port. The
listener validates the path and state, bounds request size/read time, and ignores
unrelated callbacks. Login expires after five minutes; Cancel drops the flow and
listener. Closing the account popover leaves login running, so reopening it
still offers Cancel.

The desktop sends the code, verifier, callback URI, nonce, install ID, app version
and label to `POST /v1/auth/google/verify`. The server exchanges the code with
Google, validates the signed ID token's issuer, audience, lifetime, nonce and
verified email, then keys the identity by Google's stable `sub`. It issues the
same socai device token used by SMS login. Google access/refresh/ID tokens are
not persisted, and no mailbox permissions are requested.

`POST /v1/auth/google/link` uses the same request body plus the existing socai
bearer token. The server binds the Google identity to the authenticated user and
issues a session for that user. Both endpoints return `user_id`, `device_id`,
`device_token`, `status`, `phone` (possibly empty) and `email`. SMS responses and
`GET /v1/auth/me` also return the linked email. Existing locally saved sessions
remain readable because the added email field defaults to empty.

## Database rollout

The companion server adds `google_identities` and makes `users.phone` nullable,
retaining its uniqueness for non-null phone numbers. Startup migrates PostgreSQL
with `ALTER COLUMN ... DROP NOT NULL`. For local SQLite, it transactionally
rebuilds the existing users table while preserving columns, indexes, triggers
and child foreign-key references, and verifies those references before commit.
Back up the database before deploying the schema change. Migration is idempotent;
returning to a phone-required schema would require handling Google-only users.

## Acceptance checks

- Phone login, logout and pre-existing account data continue to work.
- First Google login creates one account and one starter wallet. Repeated login
  keeps the user ID and balance, including when the displayed Google email changes.
- Linking from an existing phone account preserves its balance and subscription;
  subsequent SMS and Google login reach the same account.
- Another account cannot claim an already bound identity; disabled accounts,
  missing bearer tokens and invalid/expired identity tokens are rejected.
- Cancel, authorization denial, timeout, missing configuration and provider/network
  failures leave any existing socai session intact.
- Verify a real Google consent round trip after configuring OAuth. Mocked UI/API
  checks and successful builds alone do not establish live Google login.
