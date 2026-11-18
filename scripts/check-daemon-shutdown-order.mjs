// Use a deliberately stalled loopback WebSocket handshake, never real Chrome.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createConnection} from 'node:net';
import {spawn} from 'node:child_process';
import {mkdtemp, lstat, readFile, writeFile} from 'node:fs/promises';
import {join, resolve} from 'node:path';

const binary = resolve(process.argv[2]);
const expectation = process.argv[3];
assert(['before', 'after'].includes(expectation));
assert.notEqual(process.platform, 'win32', 'Unix socket acceptance check');
const stage = await mkdtemp('/tmp/socai-shutdown-order-');
const socket = join(stage, 'rust-daemon.sock');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const connections = new Set();
let handshakeStarted;
const handshake = new Promise(resolve => { handshakeStarted = resolve; });
const server = createServer((_, response) => response.writeHead(404).end());
server.on('upgrade', (_, connection) => {
  connections.add(connection);
  connection.on('error', () => {});
  connection.once('close', () => connections.delete(connection));
  handshakeStarted(); // Intentionally never complete the fake browser handshake.
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const child = spawn(binary, ['__daemon'], {env: {...process.env,
  SOCAI_HOME: stage, SOCAI_RUNS_DIR: join(stage, 'runs'),
  SOCAI_CDP_WS: `ws://127.0.0.1:${server.address().port}/fake-browser`,
  SOCAI_TELEMETRY: 'off', SOCAI_TELEMETRY_TASK_TEXT: 'off'},
  stdio: ['ignore', 'ignore', 'pipe']});
child.stderr.resume();
const exited = new Promise(resolve => child.once('exit', (code, signal) => resolve({code, signal})));
const deadline = setTimeout(() => child.kill('SIGTERM'), 14000);
const request = frame => new Promise((resolve, reject) => {
  const stream = createConnection(socket);
  let buffer = '';
  stream.setTimeout(11000, () => stream.destroy(Error('Isolated IPC timeout')));
  stream.once('error', reject);
  stream.once('connect', () => stream.write(JSON.stringify(frame) + '\n'));
  stream.on('data', data => {
    buffer += data;
    for (;;) {
      const end = buffer.indexOf('\n');
      if (end < 0) break;
      const result = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      if (typeof result.ok === 'boolean') { stream.end(); resolve(result); return; }
    }
  });
  stream.once('end', () => reject(Error('IPC ended before a response')));
});
try {
  // The daemon binds IPC before writing its PID. Wait for both startup markers
  // instead of racing the PID write after observing only the socket.
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await lstat(socket).catch(() => null)
        && await lstat(join(stage, 'rust-daemon.pid')).catch(() => null)) break;
    await delay(30);
  }
  assert.equal(Number(await readFile(join(stage, 'rust-daemon.pid'), 'utf8')), child.pid);
  const ping = await request({id: 'probe', site: '', command: 'ping', args: {}});
  assert(ping.ok && ping.version && ping.build_id);
  const command = request({id: 'connect-fixture', site: 'xhs', command: 'search',
    version: ping.version, build_id: ping.build_id,
    args: {query: 'isolated-fixture', preview: true, limit: 1}}).catch(() => null);
  let handshakeTimeout;
  await Promise.race([handshake, new Promise((_, reject) => {
    handshakeTimeout = setTimeout(() => reject(Error('No fake handshake: do not treat this as coverage')), 4000);
  })]).finally(() => clearTimeout(handshakeTimeout));
  const started = performance.now();
  const shutdown = await request({id: 'stop-fixture', site: '', command: 'shutdown', args: {}});
  assert(shutdown.ok);
  await delay(150);
  const endpointDuringTeardown = Boolean(await lstat(socket).catch(() => null));
  const processDuringTeardown = child.exitCode === null && child.signalCode === null;
  const exit = await exited;
  await command;
  const result = {at: new Date().toISOString(), binary, expectation, stage,
    stalledLoopbackHandshakeReached: true, shutdownAcknowledged: true,
    endpointDuringTeardown, processDuringTeardown, exit,
    elapsedMs: Math.round(performance.now() - started),
    endpointRemovedAfterExit: !(await lstat(socket).catch(() => null)),
    chromeTouched: false, installedCliChanged: false};
  await writeFile(join(stage, 'result.json'), JSON.stringify(result, null, 2) + '\n', {flag: 'wx'});
  console.log(JSON.stringify(result));
  assert(processDuringTeardown, 'The fixture must exercise a real shutdown wait');
  assert.equal(endpointDuringTeardown, expectation === 'after');
  assert.equal(exit.code, 0);
  assert(result.endpointRemovedAfterExit);
} finally {
  clearTimeout(deadline);
  for (const connection of connections) connection.destroy();
  server.close();
  if (child.exitCode === null && child.signalCode === null) { child.kill('SIGTERM'); await exited; }
}
