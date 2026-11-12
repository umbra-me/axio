"""529 twice then success; then a stuck 529 stopped with Esc during the wait."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
n = [0]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        n[0] += 1
        last = body['messages'][-1]['content'] or ''
        if n[0] <= 2 or 'stuck' in last:
            self.send_response(529); self.send_header('retry-after', '1'); self.end_headers()
            self.wfile.write(b'{"error":"overloaded"}'); return
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": "recovered"}, "finish_reason": "stop"}]})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0: os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def pump(t, until=None):
    global out
    end = time.time() + t
    while time.time() < end:
        if until and until.encode() in out: return
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
pump(1); t0 = time.time(); os.write(fd, b'hi\r'); pump(10, 'recovered')
s = out.decode(errors='replace')
ok('retried twice, honouring retry-after', 'the API is overloaded; retrying in 1s (1 of 4)' in s and '(2 of 4)' in s)
ok('recovered', 'recovered' in s and 1.8 < time.time() - t0 < 6)
mark = len(out); os.write(fd, b'stuck\r'); pump(3, '(1 of 4)'); t1 = time.time(); os.write(fd, b'\x1b'); pump(3, 'stopped')
ok('esc stops a retry wait', 'stopped' in out[mark:].decode(errors='replace') and time.time() - t1 < 1)
os.write(fd, b'\x04'); pump(1)
