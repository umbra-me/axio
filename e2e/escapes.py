"""Terminal sequences in model text, command output and /output never
reach the terminal: no clipboard write (OSC 52), no screen clear."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
EVIL = '\x1b]52;c;SGVsbG8=\x07\x1b[2J\u009d52;c;eA==\u009c'
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
            cmd = "printf 'zqa\\033]52;c;SGVsbG8=\\007\\033[2Jzqb\\n'"
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Bash', 'arguments': json.dumps({'command': cmd})}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            # Split across chunks, as a stream may deliver it.
            for part in ['said', EVIL[:5], EVIL[5:], ' done']:
                ev({'choices': [{'delta': {'content': part}}]})
            ev({'choices': [{'delta': {}, 'finish_reason': 'stop'}]})
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
pump(1); os.write(fd, b'go\r'); pump(2.5)
os.write(fd, b'/output\r'); pump(0.8)
s = out.decode(errors='replace')
ok('escapes: model text shown', 'said' in s and 'done' in s)
ok('escapes: command output shown', 'zqazqb' in s)
ok('escapes: no clipboard write', b'\x1b]52' not in out and '\u009d' not in s)
ok('escapes: no screen clear', b'\x1b[2J' not in out)
os.write(fd, b'\x04'); pump(0.5)
