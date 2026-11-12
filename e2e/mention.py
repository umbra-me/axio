"""@notes<Tab> completes, and the file's text reaches the model."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
work = os.path.join(HOME, 'w'); os.makedirs(work)
open(os.path.join(work, 'notes.md'), 'w').write('remember the milk\n')
open(os.path.join(work, 'pic.png'), 'wb').write(b'\x89PNG')
seen = []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        seen.append(json.loads(self.rfile.read(int(self.headers['content-length']))))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": "noted"}, "finish_reason": "stop"}]})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0: os.chdir(work); os.execve(BIN, [BIN, '--no-session', '--trust'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
pump(1); os.write(fd, b'summarize @no\t'); pump(0.3); os.write(fd, b'\r'); pump(2)
s = out.decode(errors='replace')
ok('tab completed the mention', 'summarize @notes.md' in s)
ok('user told it was attached', 'attached notes.md · 1 line' in s)
sent = json.dumps(seen[-1]['messages'][-1]) if seen else ''
ok('model got the file', '<file path=\\"notes.md\\">\\nremember the milk' in sent)
os.write(fd, b'look @pic.png\r'); pump(2)
sent = json.dumps(seen[-1]['messages'][-1]) if seen else ''
ok('image sent as a data URL', 'data:image/png;base64,iVBORw==' in sent)
os.write(fd, b'\x04'); pump(0.5)
