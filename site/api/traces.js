// OTLP/HTTP JSON trace proxy: socai clients POST one ExportTraceServiceRequest
// per completed agent run; we sanitize and forward it to Axiom's native OTLP
// traces endpoint so spans land parsed (trace_id/span_id/duration columns).
// Mirrors api/telemetry.js: no Axiom token in clients, best-effort forwarding.

const DEFAULT_AXIOM_URL = 'https://api.axiom.co';
const DEFAULT_DATASET = 'socai-traces-prod';
const MAX_BODY_BYTES = 512 * 1024;
const MAX_SPANS_PER_REQUEST = 500;
const MAX_ATTRIBUTES_PER_SPAN = 64;
// task_text rides in a span attribute; matches the events route's widest cap.
const MAX_STRING_CHARS = 8_000;
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
  if (countSpans(resourceSpans) > MAX_SPANS_PER_REQUEST) {
    res.status(413).json({ ok: false, error: 'too_many_spans' });
    return;
  }

  if (!consumeRateLimit(rateLimitKey(req, resourceSpans))) {
    res.status(429).json({ ok: false, error: 'rate_limited' });
    return;
  }

  sanitizeResourceSpans(resourceSpans);

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

function countSpans(resourceSpans) {
  let count = 0;
  for (const resourceSpan of resourceSpans) {
    for (const scopeSpan of scopeSpansOf(resourceSpan)) {
      if (Array.isArray(scopeSpan.spans)) {
        count += scopeSpan.spans.length;
      }
    }
  }
  return count;
}

function scopeSpansOf(resourceSpan) {
  return resourceSpan && Array.isArray(resourceSpan.scopeSpans) ? resourceSpan.scopeSpans : [];
}

function resourceAttribute(resourceSpan, key) {
  const attributes = resourceSpan?.resource?.attributes;
  if (!Array.isArray(attributes)) {
    return undefined;
  }
  const match = attributes.find((attribute) => attribute?.key === key);
  return match?.value?.stringValue;
}

// Cap attribute counts and string lengths in place. The OTLP shape itself is
// left intact — Axiom's endpoint validates the protocol; we only bound what a
// hostile client could inflate.
function sanitizeResourceSpans(resourceSpans) {
  for (const resourceSpan of resourceSpans) {
    truncateAttributes(resourceSpan?.resource?.attributes);
    for (const scopeSpan of scopeSpansOf(resourceSpan)) {
      if (!Array.isArray(scopeSpan.spans)) {
        continue;
      }
      for (const span of scopeSpan.spans) {
        if (Array.isArray(span?.attributes) && span.attributes.length > MAX_ATTRIBUTES_PER_SPAN) {
          span.attributes.length = MAX_ATTRIBUTES_PER_SPAN;
        }
        truncateAttributes(span?.attributes);
        if (typeof span?.name === 'string') {
          span.name = truncateString(span.name, 256);
        }
        if (typeof span?.status?.message === 'string') {
          span.status.message = truncateString(span.status.message, 1_000);
        }
      }
    }
  }
}

function truncateAttributes(attributes) {
  if (!Array.isArray(attributes)) {
    return;
  }
  for (const attribute of attributes) {
    const value = attribute?.value;
    if (value && typeof value.stringValue === 'string') {
      value.stringValue = truncateString(value.stringValue, MAX_STRING_CHARS);
    }
  }
}

function truncateString(value, maxChars) {
  const cleaned = value.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, '');
  return cleaned.length > maxChars ? `${cleaned.slice(0, maxChars)}…` : cleaned;
}

function rateLimitKey(req, resourceSpans) {
  const installId = resourceAttribute(resourceSpans[0], 'socai.install_id');
  if (installId) {
    return `install:${truncateString(installId, 80)}`;
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
