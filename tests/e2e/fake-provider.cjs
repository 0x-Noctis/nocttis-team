const http = require('node:http');

const host = '127.0.0.1';
const port = 7411;

function response(body) {
  return {
    id: 'fixture-response',
    object: 'chat.completion',
    created: 0,
    model: 'fixture-model',
    choices: [{ index: 0, message: body, finish_reason: body.tool_calls ? 'tool_calls' : 'stop' }],
    usage: { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 }
  };
}

function modelReply(request) {
  const messages = Array.isArray(request.messages) ? request.messages : [];
  const toolResults = messages.filter(({ role }) => role === 'tool').length;
  const reviewer = messages.some(({ content }) =>
    typeof content === 'string' && content.toLowerCase().includes('reviewer')
  );

  if (reviewer) {
    return response({ role: 'assistant', content: JSON.stringify({ decision: 'approved', findings: [] }) });
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
            patch: 'diff --git a/e2e-output.txt b/e2e-output.txt\nnew file mode 100644\n--- /dev/null\n+++ b/e2e-output.txt\n@@ -0,0 +1 @@\n+vertical smoke complete\n'
          })
        }
      }]
    });
  }
  if (toolResults === 1 && Array.isArray(request.tools) && request.tools.length > 0) {
    return response({
      role: 'assistant',
      content: null,
      tool_calls: [{
        id: 'fixture-artifact',
        type: 'function',
        function: {
          name: 'submit_artifact',
          arguments: JSON.stringify({
            path: 'e2e-output.txt',
            artifact_id: 'e2e-result',
            logical_name: 'vertical-smoke-result',
            media_type: 'text/plain'
          })
        }
      }]
    });
  }
  return response({
    role: 'assistant',
    content: JSON.stringify({
      summary: 'Fixture task completed without source mutation.',
      next_status: 'SELF_CHECK',
      changed_paths: [],
      verification_commands: ['git diff --check'],
      artifacts: []
    })
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
    result.writeHead(200, { 'content-type': 'application/json' });
    result.end(JSON.stringify(modelReply(body)));
  });
});

server.listen(port, host);
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
