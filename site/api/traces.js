// OTLP/HTTP JSON trace proxy: socai clients POST one ExportTraceServiceRequest
// per completed agent run; we forward it to Axiom's native OTLP traces
// endpoint so spans land parsed (trace_id/span_id/duration columns).
// Mirrors api/telemetry.js: no Axiom token in clients, best-effort forwarding.
//
// Transport-only by design: the proxy guards the pipe (shape gate, body-size
// cap, rate limit) and never inspects span contents. Payload shaping — which
// attributes exist, string caps, span-count bounds — is the client's
// responsibility (see RunTraceBuilder), so clients can evolve the schema
// without a proxy deploy. Same philosophy as the events route's no-allowlist
// cleanup.

const DEFAULT_AXIOM_URL = 'https://api.axiom.co';
const DEFAULT_DATASET = 'socai-traces-prod';
const MAX_BODY_BYTES = 512 * 1024;
const RATE_LIMIT_WINDOW_MS = 60_000;
const RATE_LIMIT_MAX_REQUESTS = 120;

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

  let payload;
  try {
    payload = await readJsonBody(req);
  } catch (error) {
    res.status(error.statusCode || 400).json({ ok: false, error: error.code || 'invalid_json' });
    return;
  }

  const resourceSpans = payload && typeof payload === 'object' ? payload.resourceSpans : undefined;
  if (!Array.isArray(resourceSpans) || resourceSpans.length === 0) {
    res.status(400).json({ ok: false, error: 'no_resource_spans' });
    return;
  }
  // Same spirit as the events route's `socai_` name gate: only forward traces
  // that identify as ours.
  if (resourceAttribute(resourceSpans[0], 'service.name') !== 'socai') {
    res.status(400).json({ ok: false, error: 'unknown_service' });
    return;
  }
  if (!consumeRateLimit(rateLimitKey(req, resourceSpans))) {
    res.status(429).json({ ok: false, error: 'rate_limited' });
    return;
  }

  try {
    await forwardToAxiom({ resourceSpans });
  } catch (error) {
    // Best-effort, same as events: the client has handed the trace off; proxy
    // or Axiom outages must not create retry storms or leak backend details.
    console.error('trace forward failed', error);
  }

  res.status(202).json({ ok: true });
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

function resourceAttribute(resourceSpan, key) {
  const attributes = resourceSpan?.resource?.attributes;
  if (!Array.isArray(attributes)) {
    return undefined;
  }
  const match = attributes.find((attribute) => attribute?.key === key);
  return match?.value?.stringValue;
}

function rateLimitKey(req, resourceSpans) {
  const installId = resourceAttribute(resourceSpans[0], 'socai.install_id');
  if (installId) {
    return `install:${installId.slice(0, 80)}`;
  }
  const forwardedFor = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim();
  return `ip:${forwardedFor || req.socket?.remoteAddress || 'unknown'}`;
}

function consumeRateLimit(key) {
  const now = Date.now();
  for (const [existingKey, bucket] of rateLimits) {
    if (now - bucket.startedAt > RATE_LIMIT_WINDOW_MS * 2) {
      rateLimits.delete(existingKey);
    }
  }

  const bucket = rateLimits.get(key);
  if (!bucket || now - bucket.startedAt > RATE_LIMIT_WINDOW_MS) {
    rateLimits.set(key, { startedAt: now, count: 1 });
    return true;
  }

  bucket.count += 1;
  return bucket.count <= RATE_LIMIT_MAX_REQUESTS;
}

async function forwardToAxiom(payload) {
  const token = process.env.AXIOM_TRACES_TOKEN || process.env.AXIOM_TOKEN;
  if (!token) {
    console.warn('AXIOM_TRACES_TOKEN is not configured; dropping trace');
    return;
  }

  const dataset = process.env.AXIOM_TRACES_DATASET || DEFAULT_DATASET;
  const baseUrl = (process.env.AXIOM_URL || DEFAULT_AXIOM_URL).replace(/\/+$/, '');
  const response = await fetch(`${baseUrl}/v1/traces`, {
    method: 'POST',
    headers: axiomHeaders(token, dataset),
    body: JSON.stringify(payload),
  });

  if (!response.ok) {
    const body = await response.text().catch(() => '');
    throw new Error(`Axiom trace ingest failed: ${response.status} ${body.slice(0, 300)}`);
  }
}

function axiomHeaders(token, dataset) {
  const headers = {
    Authorization: `Bearer ${token}`,
    'X-Axiom-Dataset': dataset,
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
