const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('ws');

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end('Blooket Bot Microservice Active\n');
});

const wss = new WebSocket.Server({ server });

wss.on('connection', (ws) => {
  console.log('[microservice] Client connected');
  let child = null;

  ws.on('message', (message) => {
    try {
      const data = JSON.parse(message);
      if (data.type === 'start') {
        const { pin, name, auto, headless } = data;
        
        const args = ['join_blooket.py', '--pin', pin, '--name', name];
        // Leave out troll and flood as requested by user
        if (auto && auto !== 'none' && auto !== 'troll') {
          args.push('--auto', auto);
        }
        if (headless) {
          args.push('--headless');
        }
        if (process.env.FLARESOLVERR_URL) {
          args.push('--solver-url', process.env.FLARESOLVERR_URL);
        }
        
        console.log(`[microservice] Spawning: python3 ${args.join(' ')}`);
        child = spawn('python3', args, {
          env: { ...process.env, PYTHONUNBUFFERED: '1' }
        });
        
        child.stdout.on('data', (chunk) => {
          ws.send(JSON.stringify({ type: 'stdout', data: chunk.toString() }));
        });
        
        child.stderr.on('data', (chunk) => {
          ws.send(JSON.stringify({ type: 'stderr', data: chunk.toString() }));
        });
        
        child.on('close', (code) => {
          console.log(`[microservice] Child process exited with code ${code}`);
          ws.send(JSON.stringify({ type: 'exit', code }));
          ws.close();
        });
        
        child.on('error', (err) => {
          console.error(`[microservice] Child process error:`, err);
          ws.send(JSON.stringify({ type: 'error', message: err.message }));
          ws.close();
        });
      } else if (data.type === 'input') {
        if (child && !child.killed) {
          const rawText = String(data.text ?? '');
          const safeText = rawText.trim().startsWith('{') ? ' ' + rawText : rawText;
          child.stdin.write(safeText + '\n');
        }
      } else if (data.type === 'gui-cheat' || data.type === 'eval-js') {
        if (child && !child.killed) {
          child.stdin.write(JSON.stringify(data) + '\n');
        }
      }
    } catch (e) {
      console.error('[microservice] Error handling message:', e);
    }
  });

  ws.on('close', () => {
    console.log('[microservice] Client disconnected');
    if (child && !child.killed) {
      console.log('[microservice] Killing child process due to socket disconnect');
      child.kill('SIGKILL');
    }
  });
});

const PORT = 8082;
server.listen(PORT, '0.0.0.0', () => {
  console.log(`Blooket Bot Microservice listening on port ${PORT}`);
});
