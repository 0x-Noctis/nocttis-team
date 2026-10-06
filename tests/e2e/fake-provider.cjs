const http = require('node:http');
const lead = require('../scenarios/lead/scenarios.cjs');
const parallel = require('../scenarios/parallel/scenarios.cjs');

const host = '127.0.0.1';
const port = 7411;
let calls = 0;

function response(body, tokens = 20) {
  return {
    id: 'fixture-response',
    object: 'chat.completion',
    created: 0,
    model: 'fixture-model',
    choices: [{ index: 0, message: body, finish_reason: body.tool_calls ? 'tool_calls' : 'stop' }],
    usage: { prompt_tokens: tokens - 8, completion_tokens: 8, total_tokens: tokens }
  };
}

const isProbe = (request) =>
  request.tools?.some(({ function: definition }) => definition?.name === 'noctis_capability_probe');

function modelReply(request) {
  // Request Lead dijawab lebih dulu dan tidak menambah `calls`, supaya urutan reviewer worker tidak bergeser.
  if (lead.isLeadRequest(request)) return response({ role: 'assistant', content: lead.leadReply(request) });
  const probe = request.tools?.some(({ function: definition }) =>
    definition?.name === 'noctis_capability_probe'
  );
  if (probe) {
    return response({
      role: 'assistant',
      content: null,
      tool_calls: [{
        id: 'fixture-probe',
        type: 'function',
        function: { name: 'noctis_capability_probe', arguments: '{"enabled":true}' }
      }]
    });
  }

  calls += 1;
  const messages = Array.isArray(request.messages) ? request.messages : [];
  const toolResults = messages.filter(({ role }) => role === 'tool').length;

  if (calls % 3 === 0) {
    return response({ role: 'assistant', content: JSON.stringify({ decision: 'approved' }) });
  }
  if (toolResults === 0 && Array.isArray(request.tools) && request.tools.length > 0) {
    return response({
      role: 'assistant',
      content: null,
      tool_calls: [{
        id: 'fixture-patch',
        type: 'function',
        function: {
          name: 'apply_patch',
          arguments: JSON.stringify({
            patch: 'diff --git a/e2e-output.txt b/e2e-output.txt\n--- a/e2e-output.txt\n+++ b/e2e-output.txt\n@@ -1 +1 @@\n-base\n+vertical smoke complete\n'
          })
        }
      }]
    });
  }
  return response({
    role: 'assistant',
    content: JSON.stringify({ summary: 'Fixture patch completed.', status: 'self_check' })
  });
}

const server = http.createServer((request, result) => {
  if (request.method === 'GET' && request.url === '/health') {
    result.writeHead(200, { 'content-type': 'application/json' });
    result.end('{"status":"ok"}');
    return;
  }
  if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
    result.writeHead(404).end();
    return;
  }

  let raw = '';
  request.setEncoding('utf8');
  request.on('data', (chunk) => {
    raw += chunk;
    if (raw.length > 1_048_576) request.destroy();
  });
  request.on('end', () => {
    let body;
    try {
      body = JSON.parse(raw);
    } catch {
      result.writeHead(400).end();
      return;
    }
    // Task paralel (M4-010) dijawab per task, tanpa penghitung global, dan boleh ditunda (delay) agar tumpang tindih.
    if (!isProbe(body) && !lead.isLeadRequest(body) && parallel.isParallelRequest(body)) {
      const { message, delayMs, tokens } = parallel.reply(body);
      setTimeout(() => {
        result.writeHead(200, { 'content-type': 'application/json' });
        result.end(JSON.stringify(response(message, tokens)));
      }, delayMs);
      return;
    }
    result.writeHead(200, { 'content-type': 'application/json' });
    result.end(JSON.stringify(modelReply(body)));
  });
});

server.listen(port, host);
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
