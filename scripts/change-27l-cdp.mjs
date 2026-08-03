const args = process.argv.slice(2);

function option(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}

const port = Number(option('--port', '9337'));
const expression = option('--expression');
if (!Number.isInteger(port) || port < 1 || port > 65535 || !expression) {
  console.error('Usage: node scripts/change-27l-cdp.mjs --port <port> --expression <javascript>');
  process.exit(2);
}

const controller = new AbortController();
const timeout = setTimeout(() => controller.abort(), 10_000);

try {
  const targets = await fetch(`http://127.0.0.1:${port}/json/list`, {
    signal: controller.signal,
  }).then((response) => {
    if (!response.ok) throw new Error(`CDP target list failed: HTTP ${response.status}`);
    return response.json();
  });
  const target = targets.find(
    (candidate) => candidate.type === 'page' && candidate.webSocketDebuggerUrl,
  );
  if (!target) throw new Error('CDP has no page target');

  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', () => reject(new Error('CDP WebSocket failed')), {
      once: true,
    });
  });

  const response = await new Promise((resolve, reject) => {
    const id = 1;
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    });
    socket.send(
      JSON.stringify({
        id,
        method: 'Runtime.evaluate',
        params: {
          expression,
          awaitPromise: true,
          returnByValue: true,
        },
      }),
    );
  });
  socket.close();
  if (response.exceptionDetails) {
    throw new Error(response.exceptionDetails.exception?.description ?? 'CDP evaluation failed');
  }
  console.log(JSON.stringify(response.result.value ?? null));
} finally {
  clearTimeout(timeout);
}
