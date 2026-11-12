"""A SessionStart hook's context is not a resumed conversation or a prompt."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": "fine"}, "finish_reason": "stop"}]})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
json.dump({'hooks': {'SessionStart': [{'hooks': [{'type': 'command', 'command': 'echo branch-is-main'}]}]}}, open(os.path.join(HOME, 'settings.json'), 'w'))
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
def run(args, keys):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(HOME); os.execve(BIN, [BIN] + args, env)
    out = b''
    def pump(t):
        nonlocal out
        end = time.time() + t
        while time.time() < end:
            if select.select([fd], [], [], 0.05)[0]:
                try: out += os.read(fd, 65536)
                except OSError: return
    pump(1.5)
    for k in keys: os.write(fd, k); pump(1.5)
    os.write(fd, b'\x04'); pump(1)
    return out.decode(errors='replace')
ok = lambda n, v: print(('PASS ' if v else 'FAIL ') + n)
first = run([], [b'my first question\r'])
ok('a fresh session does not say resumed', 'resumed' not in first)
again = run(['-c'], [])
ok('continuing says resumed', 'resumed' in again)
ok('the recap shows the prompt, not the hook context', '› my first question' in again and '› branch-is-main' not in again)
