// Evaluates an expression in the Electron main process over its inspector.
const [port, expression] = process.argv.slice(2);
const [target] = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate",
  params: { expression, awaitPromise: true, returnByValue: true, includeCommandLineAPI: true } }));
const reply = await new Promise((r) => ws.addEventListener("message", (m) => r(JSON.parse(m.data))));
ws.close();
const { result, exceptionDetails } = reply.result;
if (exceptionDetails) { console.error(JSON.stringify(exceptionDetails)); process.exit(1); }
console.log(JSON.stringify(result.value));
