# proxy

Node.js + Xvfb stream proxy (`stream_server.cjs`) used for browser-stream
relaying — built via `Dockerfile.proxy` and run as the `proxy` service in
the root `docker-compose.yml` (port 8081).

```bash
npm install
node stream_server.cjs
```
