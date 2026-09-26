import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';

import handler from './evidence.js';

const SCHEMA_VERSION = 'socai.model-visible-evidence.v1';
let sequence = 0;

function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

function manifestRecord(step, installId) {
  return {
    schema_version: SCHEMA_VERSION,
    record_type: 'request_manifest',
    evaluation_id: `${'a'.repeat(32)}:${'b'.repeat(16)}`,
    trace_id: 'a'.repeat(32),
    root_span_id: 'b'.repeat(16),
    run_id: 'run-test',
    provider: 'deepseek',
    model: 'deepseek-v4-pro',
    created_at: '2026-08-25T00:00:00Z',
    source: 'desktop',
    install_id: installId,
    client_version: '0.5.4',
    platform: 'macos',
    step,
    request_status: 'accepted',
    evidence_ids: [],
    evidence_count: 0,
    manifest_sha256: 'c'.repeat(64),
    error: null,
  };
}

function envelope(count) {
  sequence += 1;
  const installId = `evidence-test-${sequence}`;
  const records = Array.from({ length: count }, (_, index) => manifestRecord(index + 1, installId));
  return {
    schema_version: SCHEMA_VERSION,
    evaluation_id: records[0].evaluation_id,
    batch_index: 0,
    batch_count: 1,
    batch_sha256: sha256(JSON.stringify(records)),
    records,
  };
}

function response() {
  return {
    statusCode: 0,
    body: null,
    headers: {},
    setHeader(key, value) { this.headers[key] = value; },
    status(code) { this.statusCode = code; return this; },
    json(body) { this.body = body; return this; },
    end() { return this; },
  };
}

async function invoke(body, headers = {}) {
  const res = response();
  await handler({
    method: 'POST',
    body,
    headers: { 'x-forwarded-for': `127.0.0.${sequence + 1}`, ...headers },
    socket: {},
  }, res);
  return res;
}

test('accepts exactly 32 records and echoes the verified batch hash', { concurrency: false }, async () => {
  const body = envelope(32);
  globalThis.fetch = async (_url, options) => {
    const records = JSON.parse(options.body);
    return { ok: true, json: async () => ({ ingested: records.length, failed: 0, failures: [] }) };
  };
  process.env.AXIOM_EVIDENCE_TOKEN = 'test-only';
  const res = await invoke(body);
  assert.equal(res.statusCode, 202);
  assert.equal(res.body.accepted, 32);
  assert.equal(res.body.batch_sha256, body.batch_sha256);
});

test('rejects 33 records even when the serialized body is small', { concurrency: false }, async () => {
  const body = envelope(33);
  const res = await invoke(body);
  assert.equal(res.statusCode, 400);
  assert.equal(res.body.error, 'too_many_records');
});

test('rejects a mismatched client batch hash', { concurrency: false }, async () => {
  const body = envelope(1);
  body.batch_sha256 = 'd'.repeat(64);
  const res = await invoke(body);
  assert.equal(res.statusCode, 400);
  assert.equal(res.body.error, 'batch_hash_mismatch');
});

test('does not acknowledge an unverifiable Axiom 2xx response', { concurrency: false }, async () => {
  const body = envelope(1);
  globalThis.fetch = async () => ({ ok: true, json: async () => { throw new Error('truncated'); } });
  const originalError = console.error;
  console.error = () => {};
  const res = await invoke(body).finally(() => { console.error = originalError; });
  assert.equal(res.statusCode, 502);
  assert.equal(res.body.error, 'evidence_forward_failed');
});

test('uses wire Content-Length when Vercel pre-parses the body', { concurrency: false }, async () => {
  const body = envelope(1);
  const res = await invoke(body, { 'content-length': String(1024 * 1024 + 1) });
  assert.equal(res.statusCode, 413);
  assert.equal(res.body.error, 'body_too_large');
});
