"""Two saved sessions; axio -r lists both newest first and continues the chosen one."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
work = os.path.join(HOME, 'w'); os.makedirs(work)
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
def run(args, keys):
    pid, fd = pty.fork()
    if pid == 0: os.chdir(work); os.execve(BIN, [BIN, '--trust'] + args, env)
    out = b''
    def pump(t):
        nonlocal out
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try: out += os.read(fd, 65536)
                except OSError: return
    pump(1)
    for k in keys: os.write(fd, k); pump(1.2)
    return out.decode(errors='replace')
run([], [b'the older task\r', b'\x04'])
time.sleep(1.1)
run([], [b'the newer task\r', b'\x04'])
s = run(['-r'], [b'2\r', b'\x04'])
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
ok('lists newest first', s.find('the newer task') < s.find('the older task') and ' 1 ' in s and ' 2 ' in s)
ok('continues the chosen session', '› the older task' in s.split('Continue which?')[1])
