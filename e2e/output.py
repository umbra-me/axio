"""/output prints the whole result the tool header summarized."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
n = [0]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        n[0] += 1
        def ev(d): self.wfile.write(f'data: {json.dumps(d)}\n\n'.encode()); self.wfile.flush()
        if n[0] == 1:
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Bash', 'arguments': json.dumps({'command': 'seq 1 12 | sed s/^/row-/'})}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'done'}, 'finish_reason': 'stop'}]})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0: os.chdir(HOME); os.execve(BIN, [BIN, '--no-session'], env)
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
pump(1); os.write(fd, b'/output\r'); pump(0.5)
ok('/output before any tool', 'no tool has run yet' in out.decode(errors='replace'))
os.write(fd, b'count\r'); pump(2.5)
before = out.decode(errors='replace')
ok('header shows only the tail', 'row-12' in before and 'row-1\r' not in before)
mark = len(out); os.write(fd, b'/output\r'); pump(0.8)
s = out[mark:].decode(errors='replace')
ok('/output shows every line', all(f'row-{i}\r' in s for i in range(1, 13)) and '── Bash seq 1 12' in s)
os.write(fd, b'\x04'); pump(0.5)
