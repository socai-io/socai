const DEFAULT_AXIOM_URL = 'https://api.axiom.co';
const DEFAULT_DATASET = 'socai-cli-prod';
const MAX_BODY_BYTES = 128 * 1024;
const MAX_EVENTS_PER_REQUEST = 100;
const MAX_STRING_CHARS = 2_000;
// Per-field overrides for the default string cap. The desktop sends the full
// agent prompt as `task_text`, which is routinely longer than a search query.
const FIELD_MAX_CHARS = { task_text: 8_000 };
const MAX_METADATA_FIELDS = 20;
const MAX_METADATA_KEY_CHARS = 80;
const RATE_LIMIT_WINDOW_MS = 60_000;
const RATE_LIMIT_MAX_EVENTS = 1_200;

const rateLimits = new Map();

export default async function handler(req, res) {
  setSecurityHeaders(res);

  if (req.method === 'OPTIONS') {
    res.status(204).end();
    return;
  }

  if (req.method !== 'POST') {
    res.setHeader('Allow', 'POST, OPTIONS');
    res.status(405).json({ ok: false, error: 'method_not_allowed' });
    return;
  }

  let input;
  try {
    input = await readJsonBody(req);
  } catch (error) {
    res.status(error.statusCode || 400).json({ ok: false, error: error.code || 'invalid_json' });
    return;
  }

  const events = normalizeEvents(input);
  if (events.length === 0) {
    res.status(400).json({ ok: false, error: 'no_events' });
    return;
  }
  if (events.length > MAX_EVENTS_PER_REQUEST) {
    res.status(413).json({ ok: false, error: 'too_many_events' });
    return;
  }

  const rateKey = rateLimitKey(req, events);
  if (!consumeRateLimit(rateKey, events.length)) {
    res.status(429).json({ ok: false, error: 'rate_limited' });
    return;
  }

  const sanitized = events
    .map((event) => sanitizeEvent(event))
    .filter((event) => event !== null);

  if (sanitized.length === 0) {
    res.status(400).json({ ok: false, error: 'no_valid_events' });
    return;
  }

  try {
    await forwardToAxiom(sanitized);
  } catch (error) {
    // Keep the client contract best-effort. The CLI has already handed off the
    // batch to the proxy; proxy/Axiom outages should not create retry storms or
    // leak backend details to public clients.
    console.error('telemetry forward failed', error);
  }

  res.status(202).json({ ok: true, accepted: sanitized.length });
}

async function readJsonBody(req) {
  if (req.body !== undefined && req.body !== null) {
    return typeof req.body === 'string' ? parseJson(req.body) : req.body;
  }

  let size = 0;
  const chunks = [];
  for await (const chunk of req) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += buffer.byteLength;
    if (size > MAX_BODY_BYTES) {
      const error = new Error('body_too_large');
      error.statusCode = 413;
      error.code = 'body_too_large';
      throw error;
    }
    chunks.push(buffer);
  }

  return parseJson(Buffer.concat(chunks).toString('utf8'));
}

function parseJson(text) {
  try {
    return JSON.parse(text || 'null');
  } catch {
    const error = new Error('invalid_json');
    error.statusCode = 400;
    error.code = 'invalid_json';
    throw error;
  }
}

function normalizeEvents(input) {
  if (Array.isArray(input)) {
    return input;
  }
  if (input && typeof input === 'object' && Array.isArray(input.events)) {
    return input.events;
  }
  if (input && typeof input === 'object') {
    return [input];
  }
  return [];
}

function sanitizeEvent(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return null;
  }

  const flattened = flattenEvent(raw);
  const eventName = cleanString(flattened.event);
  if (!eventName || !eventName.startsWith('socai_')) {
    return null;
  }

  // No field allowlist: forward every key the client sent, sanitizing values
  // only (control chars, length caps, metadata shape). Non-scalar values other
  // than `metadata` are still dropped by sanitizeValue. Privacy is now the
  // client's responsibility — the proxy no longer gates which fields may be sent.
  const out = {};

  for (const [key, value] of Object.entries(flattened)) {
    const safe =
      key === 'metadata'
        ? sanitizeMetadata(value)
        : sanitizeValue(value, FIELD_MAX_CHARS[key] ?? MAX_STRING_CHARS);
    if (safe !== undefined) {
      out[key] = safe;
    }
  }

  // Stamp proxy_version after the loop so a client-sent value can't override it.
  out.proxy_version = 1;
  return out;
}

function flattenEvent(raw) {
  const flattened = { ...raw };
  if (raw.properties && typeof raw.properties === 'object' && !Array.isArray(raw.properties)) {
    for (const [key, value] of Object.entries(raw.properties)) {
      if (flattened[key] === undefined) {
        flattened[key] = value;
      }
    }
    delete flattened.properties;
  }
  if (flattened.install_id === undefined && typeof flattened.distinct_id === 'string') {
    flattened.install_id = flattened.distinct_id;
  }
  if (flattened.session_id === undefined && typeof flattened.daemon_session_id === 'string') {
    flattened.session_id = flattened.daemon_session_id;
  }
  delete flattened.daemon_session_id;
  return flattened;
}

function sanitizeMetadata(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return undefined;
  }

  const out = {};
  for (const [rawKey, rawValue] of Object.entries(value).slice(0, MAX_METADATA_FIELDS)) {
    const key = cleanMetadataKey(rawKey);
    if (!key) {
      continue;
    }
    const safe = sanitizeValue(rawValue);
    if (safe !== undefined) {
      out[key] = safe;
    }
  }

  return Object.keys(out).length > 0 ? out : undefined;
}

function sanitizeValue(value, maxChars = MAX_STRING_CHARS) {
  if (value === null) {
    return null;
  }
  switch (typeof value) {
    case 'string':
      return cleanString(value, maxChars);
    case 'number':
      return Number.isFinite(value) ? value : undefined;
    case 'boolean':
      return value;
    default:
      return undefined;
  }
}

function cleanMetadataKey(value) {
  if (typeof value !== 'string') {
    return undefined;
  }
  const key = value.trim().slice(0, MAX_METADATA_KEY_CHARS);
  if (!/^[A-Za-z0-9_.-]+$/.test(key)) {
    return undefined;
  }
  return key;
}

function cleanString(value, maxChars = MAX_STRING_CHARS) {
  if (typeof value !== 'string') {
    return undefined;
  }
  const cleaned = value.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, '').trim();
  return cleaned.length > maxChars ? `${cleaned.slice(0, maxChars)}…` : cleaned;
}

function rateLimitKey(req, events) {
  const installId = events
    .map(flattenEvent)
    .map((event) => cleanString(event.install_id || event.distinct_id))
    .find(Boolean);
  if (installId) {
    return `install:${installId}`;
  }
  const forwardedFor = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim();
  return `ip:${forwardedFor || req.socket?.remoteAddress || 'unknown'}`;
}

function consumeRateLimit(key, count) {
  const now = Date.now();
  for (const [existingKey, bucket] of rateLimits) {
    if (now - bucket.startedAt > RATE_LIMIT_WINDOW_MS * 2) {
      rateLimits.delete(existingKey);
    }
  }

  const bucket = rateLimits.get(key);
  if (!bucket || now - bucket.startedAt > RATE_LIMIT_WINDOW_MS) {
    rateLimits.set(key, { startedAt: now, count });
    return count <= RATE_LIMIT_MAX_EVENTS;
  }

  bucket.count += count;
  return bucket.count <= RATE_LIMIT_MAX_EVENTS;
}

async function forwardToAxiom(events) {
  const token = process.env.AXIOM_TOKEN;
  if (!token) {
    console.warn('AXIOM_TOKEN is not configured; dropping telemetry batch');
    return;
  }

  const dataset = process.env.AXIOM_DATASET || DEFAULT_DATASET;
  const baseUrl = (process.env.AXIOM_URL || DEFAULT_AXIOM_URL).replace(/\/+$/, '');
  const response = await fetch(`${baseUrl}/v1/datasets/${encodeURIComponent(dataset)}/ingest`, {
    method: 'POST',
    headers: axiomHeaders(token),
    body: JSON.stringify(events),
  });

  if (!response.ok) {
    const body = await response.text().catch(() => '');
    throw new Error(`Axiom ingest failed: ${response.status} ${body.slice(0, 300)}`);
  }
}

function axiomHeaders(token) {
  const headers = {
    Authorization: `Bearer ${token}`,
    'Content-Type': 'application/json',
  };
  if (process.env.AXIOM_ORG_ID) {
    headers['X-Axiom-Org-ID'] = process.env.AXIOM_ORG_ID;
  }
  return headers;
}

function setSecurityHeaders(res) {
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
  res.setHeader('Cache-Control', 'no-store');
}
