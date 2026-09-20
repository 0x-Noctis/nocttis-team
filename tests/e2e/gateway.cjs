const http = require('node:http');

const server = http.createServer((request, response) => {
  const api = request.url.startsWith('/api/');
  const upstream = http.request({
    hostname: '127.0.0.1',
    port: api ? 7410 : 4174,
    method: request.method,
    path: request.url,
    headers: { ...request.headers, host: `127.0.0.1:${api ? 7410 : 4174}` }
  }, (upstreamResponse) => {
    response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers);
    upstreamResponse.pipe(response);
  });
  upstream.on('error', () => {
    if (!response.headersSent) response.writeHead(502);
    response.end();
  });
  request.pipe(upstream);
});

server.listen(4173, '127.0.0.1');
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
