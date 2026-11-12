"""-p with a prompt argument must not hang on a stdin that stays open, and must still read late input."""
import http.server, json, os, subprocess, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
seen = []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length']))); seen.append(json.dumps(body['messages']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1')
ok = lambda n, v: print(('PASS ' if v else 'FAIL ') + n)
# stdin open, nothing ever written
t = time.time()
import tempfile
fo, fe = tempfile.TemporaryFile(), tempfile.TemporaryFile()
p = subprocess.Popen([BIN, '--no-session', '-p', 'hello there'], stdin=subprocess.PIPE, stdout=fo, stderr=fe, env=env, cwd=HOME)
try:
    p.wait(timeout=15); took = time.time() - t  # stdin stays open: nothing is written or closed
except subprocess.TimeoutExpired:
    p.kill(); p.wait(); took = 99
fo.seek(0); fe.seek(0); out, err = fo.read(), fe.read()
ok(f'an open, silent stdin does not hang ({took:.1f}s)', took < 10 and b'ok' in out)
ok('it says stdin was skipped', b'stdin' in err)
# input that arrives after a second is still read
p = subprocess.Popen([BIN, '--no-session', '-p', 'review this'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=HOME)
time.sleep(1); p.stdin.write(b'LATE-DIFF-TEXT\n'); p.stdin.close()
p.wait(timeout=15)
ok('late piped input is still read', 'LATE-DIFF-TEXT' in seen[-1] and 'review this' in seen[-1])
