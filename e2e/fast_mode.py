"""/fast: toggles fast mode, which reaches the request only on models that have it."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
seen = []
EVENTS = [
    {"type": "message_start", "message": {"usage": {"input_tokens": 1, "output_tokens": 0}}},
    {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
    {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}},
    {"type": "content_block_stop", "index": 0},
    {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}},
    {"type": "message_stop"},
]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        seen.append((body['model'], body.get('speed'), self.headers.get('anthropic-beta')))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        for e in EVENTS:
            self.wfile.write(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n".encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='anthropic/claude-opus-4-8', ANTHROPIC_API_KEY='k',
           ANTHROPIC_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(HOME); os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
def send(b, t=1.0):
    os.write(fd, b); pump(t)
pump(1.5)
send(b'/fast\r'); on = out.decode(errors='replace')
send(b'one\r', 2)
send(b'/fast\r'); off = out.decode(errors='replace')
send(b'two\r', 2)
send(b'/model anthropic/claude-sonnet-5-5\r')
send(b'/fast\r'); refused = out.decode(errors='replace')
send(b'three\r', 2)
send(b'\x04', 0.5)
beta = 'fast-mode-2026-02-01'
checks = [
    ('/fast says it is on, with the price', 'fast mode on for this session' in on and '$10 / $50' in on and 'research preview' in on),
    ('a fast request sends speed and the preview header', seen[0:1] == [('claude-opus-4-8', 'fast', beta)]),
    ('/fast again turns it off', 'fast mode off for this session' in off),
    ('then requests go at standard speed', seen[1:2] == [('claude-opus-4-8', None, None)]),
    ('a model without fast mode says so', 'fast mode is not available for anthropic/claude-sonnet-5-5' in refused),
    ('and sends no speed', seen[2:3] == [('claude-sonnet-5-5', None, None)]),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(seen); print(out.decode(errors='replace')[-1500:])
