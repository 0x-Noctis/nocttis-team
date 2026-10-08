// Test penormal SSE. Semua data SINTETIS (bentuk chunk OpenAI, bukan keluaran model sungguhan).
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { normalize } = require('./sse-normalize.cjs');

const chunk = (delta, extra = {}) => `data: ${JSON.stringify({ id: 'c1', created: 1, model: 'm', choices: [{ index: 0, delta, finish_reason: extra.finish ?? null }], ...(extra.usage ? { usage: extra.usage } : {}) })}\n\n`;

test('non-streaming: SSE router diagregasi menjadi JSON chat.completion lengkap dengan usage', () => {
  const sse = chunk({ content: 'O' }) + chunk({ content: 'K' }) + chunk({}, { finish: 'stop', usage: { prompt_tokens: 11, completion_tokens: 2 } });
  const result = normalize(false, 'text/event-stream', sse);
  assert.equal(result.contentType, 'application/json');
  const body = JSON.parse(result.body);
  assert.equal(body.object, 'chat.completion');
  assert.equal(body.choices[0].message.content, 'OK');
  assert.equal(body.choices[0].finish_reason, 'stop');
  assert.deepEqual(body.usage, { prompt_tokens: 11, completion_tokens: 2 });
});

test('non-streaming: tool_calls yang terpecah per chunk digabung per index, argumen utuh', () => {
  const sse =
    chunk({ tool_calls: [{ index: 0, id: 'call_a', function: { name: 'read_', arguments: '{"pa' } }] }) +
    chunk({ tool_calls: [{ index: 0, function: { name: 'file', arguments: 'th":"a"}' } }, { index: 1, id: 'call_b', function: { name: 'list', arguments: '{}' } }] }) +
    chunk({}, { finish: 'tool_calls' });
  const message = JSON.parse(normalize(false, '', sse).body).choices[0].message;
  assert.equal(message.content, null);
  assert.deepEqual(message.tool_calls.map((c) => [c.id, c.function.name, c.function.arguments]), [
    ['call_a', 'read_file', '{"path":"a"}'],
    ['call_b', 'list', '{}']
  ]);
});

test('streaming: [DONE] ditambahkan sekali bila hilang, dan tidak digandakan bila sudah ada', () => {
  const sse = chunk({ content: 'x' }) + chunk({}, { finish: 'stop' });
  const added = normalize(true, 'text/event-stream', sse);
  assert.equal(added.body.match(/\[DONE\]/g).length, 1);
  assert.ok(added.body.trimEnd().endsWith('data: [DONE]'));
  const already = normalize(true, 'text/event-stream', `${sse}data: [DONE]\n\n`);
  assert.equal(already.body.match(/\[DONE\]/g).length, 1);
});

test('respons yang sudah sesuai kontrak atau error tidak diubah', () => {
  const json = JSON.stringify({ object: 'chat.completion', choices: [] });
  assert.deepEqual(normalize(false, 'application/json', json), { contentType: 'application/json', body: json });
  const error = JSON.stringify({ error: { message: 'bad' } });
  assert.deepEqual(normalize(true, 'application/json', error), { contentType: 'application/json', body: error });
});
