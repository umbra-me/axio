"""The status line under the prompt: model, context and an estimated cost; Ctrl-O shows the last output."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
EVENTS = [
    {"type": "message_start", "message": {"usage": {"input_tokens": 100000, "output_tokens": 0}}},
    {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
    {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}},
    {"type": "content_block_stop", "index": 0},
    {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10000}},
    {"type": "message_stop"},
]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        for e in EVENTS:
            self.wfile.write(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n".encode())
            self.wfile.flush()
            if e['type'] == 'content_block_delta':
                time.sleep(0.4)  # so the reply streams long enough to time
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='anthropic/claude-opus-5-5', ANTHROPIC_API_KEY='k',
           ANTHROPIC_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm', COLUMNS='120')
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
def step(k, t=1.5):
    start = len(out); os.write(fd, k); pump(t); return out[start:].decode(errors='replace')
pump(2)
first = out.decode(errors='replace')
after = step(b'hi\r', 2.5)
ctrl_o = step(b'\x0f')
step(b'\x04', 0.5)
checks = [
    ('the status line shows the model and $0.00 at the start', 'claude-opus-5-5' in first and '% context · $0.00' in first),
    ('after a turn it shows the estimated cost', '~$0.60' in after),
    ('Ctrl-O on an empty prompt shows the last output', 'no tool has run yet' in ctrl_o),
    ('the line after a turn shows the output speed', 'tok/s · first token' in after),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(out.decode(errors='replace')[-1500:])
