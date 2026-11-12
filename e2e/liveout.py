"""A slow Bash command: its newest line shows beside the spinner while it runs."""
import http.server, json, os, pty, re, select, sys, threading, time
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
            args = json.dumps({'command': 'for i in 1 2 3 4; do echo "compiling crate $i"; sleep 0.4; done'})
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Bash', 'arguments': args}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'built'}, 'finish_reason': 'stop'}]})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', TERM='xterm-256color', COLUMNS='100')
env.pop('NO_COLOR', None)
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
pump(1); os.write(fd, b'build\r'); pump(4); os.write(fd, b'\x04'); pump(1)
s = out.decode(errors='replace')
notes = sorted(set(re.findall(r'· (compiling crate \d)', s)))
print(('PASS' if len(notes) >= 3 else 'FAIL') + f' live command output in the spinner {notes}')
print(('PASS' if 'built' in s else 'FAIL') + ' turn finished')
