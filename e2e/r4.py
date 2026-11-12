"""Numbered Edit diff, Ctrl-R search and the exit summary, in a pty."""
import http.server, json, os, pty, re, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
work = os.path.join(HOME, 'w'); os.makedirs(work)
src = os.path.join(work, 'a.py')
open(src, 'w').write('def f():\n    x = 1\n    return x\n')
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
            args = json.dumps({'file_path': src, 'old_string': 'x = 1', 'new_string': 'x = 2'})
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Edit', 'arguments': args}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'done'}, 'finish_reason': 'stop'}]})
        ev({'choices': [], 'usage': {'prompt_tokens': 10, 'completion_tokens': 7}})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
os.makedirs(os.path.join(HOME, 'agent'), exist_ok=True)
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(BIN, [BIN, '--trust'], env)
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
pump(1); os.write(fd, b'bump x\r'); pump(3)
s = out.decode(errors='replace')
ok('numbered diff with context', all(t in s for t in ['1   def f():', '2 -     x = 1', '2 +     x = 2', '3       return x']))
mark = len(out)
os.write(fd, b'\x12bu'); pump(0.5)
ok('ctrl-r finds the prompt', 'search history: bu  › bump x' in out[mark:].decode(errors='replace'))
os.write(fd, b'\x1b'); pump(0.5)
os.write(fd, b'\x04'); pump(1)
s = out.decode(errors='replace')
ok('exit summary', re.search(r'1 turn · 14 out · \d+s · `axio -c` continues this session', s) is not None)
