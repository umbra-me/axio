"""Shrink the terminal while the spinner runs: its lines must fit the new width."""
import fcntl, http.server, json, os, pty, re, select, struct, sys, termios, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    n = 0
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        Stub.n += 1
        def ev(d): self.wfile.write(f'data: {json.dumps(d)}\n\n'.encode()); self.wfile.flush()
        if Stub.n == 1:
            cmd = 'for i in $(seq 1 12); do echo "a fairly long line of build output number $i that goes on and on"; sleep 0.25; done'
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Bash', 'arguments': json.dumps({'command': cmd})}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'ok'}, 'finish_reason': 'stop'}]})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', TERM='xterm-256color')
env.pop('NO_COLOR', None); env.pop('COLUMNS', None)
pid, fd = pty.fork()
if pid == 0: os.chdir(HOME); os.execve(BIN, [BIN, '--no-session'], env)
size = lambda cols: fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 30, cols, 0, 0))
size(150)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
pump(1); os.write(fd, b'go\r'); pump(1.2)
size(50); mark = len(out); pump(1.5)
frames = re.findall(r'\r\x1b\[K([^\r\n]*)', out[mark:].decode(errors='replace'))
visible = [len(re.sub(r'\x1b\[[0-9;]*m', '', f)) for f in frames if f]
print(('PASS' if visible and max(visible) <= 50 else 'FAIL') + f' spinner fits after shrinking to 50 (widest {max(visible) if visible else None})')
pump(2.5); os.write(fd, b'\x04'); pump(0.5)
