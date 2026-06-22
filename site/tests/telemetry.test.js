import assert from 'node:assert/strict';
import test from 'node:test';

import { __testing } from '../api/telemetry.js';

test('sanitizeEvent strips fields that should not reach Axiom', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_tool_call',
    install_id: 'install-1',
    daemon_session_id: 'legacy-session-1',
    request_id: 'request-1',
    command: 'topic_scan',
    tool_name: 'topic_scan',
    query_text_enabled: false,
    metadata: {
      num_notes: 12,
      tab: 'latest',
      debug_snapshot: true,
    },
    arch: 'arm64',
    created_at_ms: 1,
    client_created_at_ms: 2,
    received_at_ms: 3,
    query_redacted: true,
    num_notes: 99,
    tab_label: 'should_drop',
  });

  assert.equal(sanitized.install_id, 'install-1');
  assert.equal(sanitized.session_id, 'legacy-session-1');
  assert.equal(sanitized.request_id, 'request-1');
  assert.equal(sanitized.command, 'topic_scan');
  assert.equal(sanitized.tool_name, 'topic_scan');
  assert.deepEqual(sanitized.metadata, {
    num_notes: 12,
    tab: 'latest',
    debug_snapshot: true,
  });
  // `event` is the type discriminator and is now forwarded (source carries the surface).
  assert.equal(sanitized.event, 'socai_tool_call');

  for (const key of [
    'arch',
    'created_at_ms',
    'client_created_at_ms',
    'received_at_ms',
    'query_redacted',
    'num_notes',
    'tab_label',
    'daemon_session_id',
  ]) {
    assert.equal(Object.hasOwn(sanitized, key), false, `${key} should be stripped`);
  }
});

test('sanitizeEvent preserves shallow primitive metadata and rejects unsafe metadata', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_tool_call',
    install_id: 'install-1',
    command: 'topic_scan',
    metadata: {
      num_notes: 5,
      tab: ' latest ',
      enabled: true,
      null_value: null,
      nested: { unsafe: true },
      array: ['unsafe'],
      nan_value: Number.NaN,
      'bad key': 'unsafe',
      'bad$key': 'unsafe',
    },
  });

  assert.deepEqual(sanitized.metadata, {
    num_notes: 5,
    tab: 'latest',
    enabled: true,
    null_value: null,
  });
});

test('sanitizeEvent rejects missing or non-socai event names', () => {
  assert.equal(__testing.sanitizeEvent({ command: 'topic_scan' }), null);
  assert.equal(
    __testing.sanitizeEvent({ event: 'other_event', command: 'topic_scan' }),
    null,
  );
});

test('sanitizeEvent flattens legacy properties without leaking disallowed fields', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_tool_call',
    distinct_id: 'install-from-distinct',
    properties: {
      request_id: 'request-1',
      command: 'search_notes',
      query_text: '  Bloc1 V4  ',
      created_at_ms: 123,
      metadata: { tab: 'discover' },
    },
  });

  assert.equal(sanitized.install_id, 'install-from-distinct');
  assert.equal(sanitized.request_id, 'request-1');
  assert.equal(sanitized.command, 'search_notes');
  assert.equal(sanitized.query_text, 'Bloc1 V4');
  assert.deepEqual(sanitized.metadata, { tab: 'discover' });
  assert.equal(sanitized.event, 'socai_tool_call');
  assert.equal(Object.hasOwn(sanitized, 'created_at_ms'), false);
});

test('sanitizeEvent keeps desktop agent-task fields', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_agent_task_end',
    install_id: 'install-1',
    source: 'desktop',
    task_id: 'task-1730000000000-3',
    run_id: '20260621-101010-000042',
    provider: 'anthropic',
    model: 'claude-sonnet-4-6',
    outcome: 'completed',
    turns: 7,
    input_tokens: 1234,
    output_tokens: 567,
    duration_ms: 42130,
    tool_name: 'topic_scan',
    turn: 3,
    sequence: 5,
    ok: true,
  });

  assert.equal(sanitized.event, 'socai_agent_task_end');
  assert.equal(sanitized.source, 'desktop');
  assert.equal(sanitized.task_id, 'task-1730000000000-3');
  assert.equal(sanitized.run_id, '20260621-101010-000042');
  assert.equal(sanitized.provider, 'anthropic');
  assert.equal(sanitized.model, 'claude-sonnet-4-6');
  assert.equal(sanitized.outcome, 'completed');
  assert.equal(sanitized.turns, 7);
  assert.equal(sanitized.input_tokens, 1234);
  assert.equal(sanitized.output_tokens, 567);
  assert.equal(sanitized.turn, 3);
  assert.equal(sanitized.sequence, 5);
  assert.equal(sanitized.duration_ms, 42130);
  assert.equal(sanitized.tool_name, 'topic_scan');
  assert.equal(sanitized.ok, true);
});

test('sanitizeEvent keeps desktop app_open setup fields', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_app_open',
    install_id: 'install-1',
    source: 'desktop',
    has_api_key: true,
    default_provider: 'anthropic',
    default_model: 'claude-sonnet-4-6',
  });

  assert.equal(sanitized.has_api_key, true);
  assert.equal(sanitized.default_provider, 'anthropic');
  assert.equal(sanitized.default_model, 'claude-sonnet-4-6');
});

test('sanitizeEvent allows task_text up to the 8000-char cap', () => {
  const longPrompt = 'x'.repeat(5000);
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_agent_task_start',
    install_id: 'install-1',
    task_text: longPrompt,
    task_len: 5000,
  });
  // 5000 chars survives intact — the default 2000 cap would have clipped it.
  assert.equal(sanitized.task_text, longPrompt);
  assert.equal(sanitized.task_len, 5000);

  const hugePrompt = 'y'.repeat(9000);
  const clipped = __testing.sanitizeEvent({
    event: 'socai_agent_task_start',
    install_id: 'install-1',
    task_text: hugePrompt,
  });
  // Beyond 8000 it is truncated to 8000 chars plus a single ellipsis.
  assert.equal(clipped.task_text.length, 8001);
  assert.ok(clipped.task_text.endsWith('…'));
});

test('sanitizeEvent strips unknown desktop fields so result bodies never leak', () => {
  const sanitized = __testing.sanitizeEvent({
    event: 'socai_agent_task_end',
    install_id: 'install-1',
    task_id: 'task-1',
    final_text: 'report body that must not reach axiom',
    assistant_text: 'model output that must not reach axiom',
  });

  assert.equal(sanitized.task_id, 'task-1');
  assert.equal(Object.hasOwn(sanitized, 'final_text'), false);
  assert.equal(Object.hasOwn(sanitized, 'assistant_text'), false);
});
