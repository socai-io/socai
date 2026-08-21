import { createHash } from 'node:crypto';

// Lossless model-visible ToolResult proxy. The client has already applied its
// chat privacy gate and secret redactor; this endpoint validates and forwards
// records without trimming or reshaping evidence content.
const SCHEMA_VERSION = 'socai.model-visible-evidence.v1';
const DEFAULT_AXIOM_URL = 'https://api.axiom.co';
const DEFAULT_DATASET = 'socai-evidence-prod';
const MAX_BODY_BYTES = 1024 * 1024;
const MAX_RECORDS = 32;
const MAX_CHUNK_BYTES = 32 * 1024;
const RATE_LIMIT_WINDOW_MS = 60_000;
const RATE_LIMIT_MAX_REQUESTS = 240;
const RATE_LIMIT_MAX_BYTES = 32 * 1024 * 1024;
const COMMON_FIELDS = [
  'schema_version', 'record_type', 'evaluation_id', 'trace_id', 'root_span_id',
  'run_id', 'provider', 'model', 'created_at', 'source', 'install_id',
  'app_session_id', 'app_version', 'platform',
];
const RECORD_FIELDS = {
  evidence_chunk: [
    'evidence_id', 'tool_call_id', 'tool_name', 'wire_format', 'first_observed_step',
    'message_index', 'result_index', 'content_encoding', 'content_sha256',
    'content_bytes', 'chunk_index', 'chunk_count', 'chunk_sha256', 'chunk_bytes',
    'chunk_text', 'redaction_version', 'redaction_count', 'semantic_redaction',
  ],
  request_manifest: [
    'step', 'request_status', 'evidence_ids', 'evidence_count', 'manifest_sha256', 'error',
  ],
  turn_commit: [
    'archive_status', 'accepted_request_count', 'evidence_object_count',
    'evidence_chunk_count', 'total_content_bytes', 'evidence_index_sha256',
    'telemetry_policy', 'committed_at',
  ],
};

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
  let bodyBytes;
  try {
    ({ input, bodyBytes } = await readJsonBody(req));
  } catch (error) {
    res.status(error.statusCode || 400).json({ ok: false, error: error.code || 'invalid_json' });
    return;
  }

  const error = validateEnvelope(input);
  if (error) {
    res.status(400).json({ ok: false, error });
    return;
  }
  const records = input.records;
  if (!consumeRateLimit(rateLimitKey(req, records), bodyBytes)) {
    res.status(429).json({ ok: false, error: 'rate_limited' });
    return;
  }

  try {
    await forwardToAxiom(records);
    res.status(202).json({
      ok: true,
      accepted: records.length,
      batch_sha256: sha256(JSON.stringify(records)),
    });
  } catch (error) {
    console.error('evidence forward failed', error instanceof Error ? error.message : 'unknown');
    res.status(502).json({ ok: false, error: 'evidence_forward_failed' });
  }
}

async function readJsonBody(req) {
  if (req.body !== undefined && req.body !== null) {
    const text = typeof req.body === 'string' ? req.body : JSON.stringify(req.body);
    const bodyBytes = Buffer.byteLength(text, 'utf8');
    if (bodyBytes > MAX_BODY_BYTES) {
      throw requestError(413, 'body_too_large');
    }
    return { input: parseJson(text), bodyBytes };
  }

  let bodyBytes = 0;
  const chunks = [];
  for await (const chunk of req) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    bodyBytes += buffer.byteLength;
    if (bodyBytes > MAX_BODY_BYTES) {
      throw requestError(413, 'body_too_large');
    }
    chunks.push(buffer);
  }
  return { input: parseJson(Buffer.concat(chunks).toString('utf8')), bodyBytes };
}

function parseJson(text) {
  try {
    return JSON.parse(text || 'null');
  } catch {
    throw requestError(400, 'invalid_json');
  }
}

function requestError(statusCode, code) {
  const error = new Error(code);
  error.statusCode = statusCode;
  error.code = code;
  return error;
}

function validateEnvelope(input) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return 'invalid_envelope';
  if (input.schema_version !== SCHEMA_VERSION) return 'unsupported_schema';
  if (!validEvaluationId(input.evaluation_id)) return 'invalid_evaluation_id';
  if (!Array.isArray(input.records) || input.records.length === 0) return 'no_records';
  if (input.records.length > MAX_RECORDS) return 'too_many_records';

  const installIds = new Set(input.records.map((record) => record?.install_id));
  const sources = new Set(input.records.map((record) => record?.source));
  if (installIds.size !== 1) return 'mixed_install_ids';
  if (sources.size !== 1) return 'mixed_sources';

  for (const record of input.records) {
    const error = validateRecord(record, input.evaluation_id);
    if (error) return error;
  }
  return null;
}

function validateRecord(record, evaluationId) {
  if (!record || typeof record !== 'object' || Array.isArray(record)) return 'invalid_record';
  if (record.schema_version !== SCHEMA_VERSION) return 'record_schema_mismatch';
  if (record.evaluation_id !== evaluationId) return 'record_evaluation_mismatch';
  if (`${record.trace_id}:${record.root_span_id}` !== evaluationId) return 'record_identity_mismatch';
  if (!validScalar(record.run_id, 200)) return 'invalid_run_id';
  if (!validScalar(record.install_id, 200)) return 'invalid_install_id';
  if (!validScalar(record.source, 80)) return 'invalid_source';
  if (!validScalar(record.provider, 100)) return 'invalid_provider';
  if (!validScalar(record.model, 200)) return 'invalid_model';

  const specificFields = RECORD_FIELDS[record.record_type];
  if (!specificFields) return 'unknown_record_type';
  const allowedFields = new Set([...COMMON_FIELDS, ...specificFields]);
  if (Object.keys(record).some((field) => !allowedFields.has(field))) return 'unknown_record_field';

  if (record.record_type === 'evidence_chunk') return validateChunk(record);
  if (record.record_type === 'request_manifest') return validateManifest(record);
  if (record.record_type === 'turn_commit') return validateCommit(record);
  return null;
}

function validateChunk(record) {
  if (!/^ev_[a-f0-9]{64}$/.test(record.evidence_id || '')) return 'invalid_evidence_id';
  if (!validScalar(record.tool_call_id, 300)) return 'invalid_tool_call_id';
  if (record.tool_name !== null && record.tool_name !== undefined && !validScalar(record.tool_name, 200)) {
    return 'invalid_tool_name';
  }
  if (!/^[a-f0-9]{64}$/.test(record.content_sha256 || '')) return 'invalid_content_hash';
  if (!/^[a-f0-9]{64}$/.test(record.chunk_sha256 || '')) return 'invalid_chunk_hash';
  if (!Number.isInteger(record.chunk_index) || record.chunk_index < 0) return 'invalid_chunk_index';
  if (!Number.isInteger(record.chunk_count) || record.chunk_count < 1) return 'invalid_chunk_count';
  if (record.chunk_count > 4096) return 'chunk_count_too_large';
  if (record.chunk_index >= record.chunk_count) return 'chunk_index_out_of_range';
  if (!Number.isInteger(record.content_bytes) || record.content_bytes < 0) return 'invalid_content_bytes';
  if (!Number.isInteger(record.first_observed_step) || record.first_observed_step < 1) {
    return 'invalid_first_observed_step';
  }
  if (!validScalar(record.wire_format, 100)) return 'invalid_wire_format';
  if (typeof record.chunk_text !== 'string') return 'invalid_chunk_text';
  const actualBytes = Buffer.byteLength(record.chunk_text, 'utf8');
  if (actualBytes > MAX_CHUNK_BYTES) return 'chunk_too_large';
  if (record.chunk_bytes !== actualBytes) return 'chunk_byte_mismatch';
  if (sha256(record.chunk_text) !== record.chunk_sha256) return 'chunk_hash_mismatch';
  return null;
}

function validateManifest(record) {
  if (!Number.isInteger(record.step) || record.step < 1) return 'invalid_request_step';
  if (!['accepted', 'failed', 'unknown', 'unsupported'].includes(record.request_status)) {
    return 'invalid_request_status';
  }
  if (!Array.isArray(record.evidence_ids) || record.evidence_ids.length > 500) {
    return 'invalid_manifest_evidence_ids';
  }
  if (!record.evidence_ids.every((value) => /^ev_[a-f0-9]{64}$/.test(value))) {
    return 'invalid_manifest_evidence_id';
  }
  if (record.evidence_count !== record.evidence_ids.length) return 'manifest_count_mismatch';
  if (!/^[a-f0-9]{64}$/.test(record.manifest_sha256 || '')) return 'invalid_manifest_hash';
  return null;
}

function validateCommit(record) {
  if (!['complete', 'partial', 'disabled'].includes(record.archive_status)) {
    return 'invalid_archive_status';
  }
  for (const field of [
    'accepted_request_count',
    'evidence_object_count',
    'evidence_chunk_count',
    'total_content_bytes',
  ]) {
    if (!Number.isInteger(record[field]) || record[field] < 0) return `invalid_${field}`;
  }
  if (!/^[a-f0-9]{64}$/.test(record.evidence_index_sha256 || '')) return 'invalid_index_hash';
  return null;
}

function validEvaluationId(value) {
  return /^[a-f0-9]{32}:[a-f0-9]{16}$/.test(value || '');
}

function validScalar(value, maxLength) {
  return typeof value === 'string' && value.length > 0 && value.length <= maxLength;
}

function sha256(text) {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function rateLimitKey(req, records) {
  const installId = records.find((record) => validScalar(record.install_id, 200))?.install_id;
  if (installId) return `install:${installId}`;
  const forwardedFor = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim();
  return `ip:${forwardedFor || req.socket?.remoteAddress || 'unknown'}`;
}

function consumeRateLimit(key, bytes) {
  const now = Date.now();
  for (const [existingKey, bucket] of rateLimits) {
    if (now - bucket.startedAt > RATE_LIMIT_WINDOW_MS * 2) rateLimits.delete(existingKey);
  }
  let bucket = rateLimits.get(key);
  if (!bucket || now - bucket.startedAt > RATE_LIMIT_WINDOW_MS) {
    bucket = { startedAt: now, requests: 0, bytes: 0 };
    rateLimits.set(key, bucket);
  }
  bucket.requests += 1;
  bucket.bytes += bytes;
  return bucket.requests <= RATE_LIMIT_MAX_REQUESTS && bucket.bytes <= RATE_LIMIT_MAX_BYTES;
}

async function forwardToAxiom(records) {
  const token = process.env.AXIOM_EVIDENCE_TOKEN;
  if (!token) throw new Error('AXIOM_EVIDENCE_TOKEN is not configured');

  const dataset = process.env.AXIOM_EVIDENCE_DATASET || DEFAULT_DATASET;
  const baseUrl = (
    process.env.AXIOM_EVIDENCE_URL || process.env.AXIOM_URL || DEFAULT_AXIOM_URL
  ).replace(/\/+$/, '');
  const response = await fetch(`${baseUrl}/v1/datasets/${encodeURIComponent(dataset)}/ingest`, {
    method: 'POST',
    headers: axiomHeaders(token),
    body: JSON.stringify(records),
  });
  if (!response.ok) throw new Error(`Axiom evidence ingest failed: ${response.status}`);

  const result = await response.json().catch(() => null);
  if (result && (result.failed > 0 || (Number.isInteger(result.ingested) && result.ingested !== records.length))) {
    throw new Error('Axiom evidence ingest was partial');
  }
}

function axiomHeaders(token) {
  const headers = {
    Authorization: `Bearer ${token}`,
    'Content-Type': 'application/json',
  };
  if (process.env.AXIOM_ORG_ID) headers['X-Axiom-Org-ID'] = process.env.AXIOM_ORG_ID;
  return headers;
}

function setSecurityHeaders(res) {
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
  res.setHeader('Cache-Control', 'no-store');
}
