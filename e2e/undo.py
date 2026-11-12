"""Edit + Write through the stub, /undo puts both back, the model hears about it."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
work = os.path.join(HOME, 'w'); os.makedirs(work)
src, new = os.path.join(work, 'a.py'), os.path.join(work, 'b.py')
open(src, 'w').write('x = 1\n')
n, seen = [0], []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        seen.append(json.dumps(body['messages']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        n[0] += 1
        def ev(d): self.wfile.write(f'data: {json.dumps(d)}\n\n'.encode()); self.wfile.flush()
        if n[0] == 1:
            calls = [{'index': 0, 'id': 'c1', 'function': {'name': 'Edit', 'arguments': json.dumps({'file_path': src, 'old_string': 'x = 1', 'new_string': 'x = 2'})}},
                     {'index': 1, 'id': 'c2', 'function': {'name': 'Write', 'arguments': json.dumps({'file_path': new, 'content': 'y = 3\n'})}}]
            ev({'choices': [{'delta': {'tool_calls': calls}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'ok'}, 'finish_reason': 'stop'}]})
        self.wfile.write(b'data: [DONE]\n\n')
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
pump(1); os.write(fd, b'change\r'); pump(3)
ok('turn changed both files', open(src).read() == 'x = 2\n' and os.path.exists(new))
os.write(fd, b'/undo\r'); pump(1)
s = out.decode(errors='replace')
ok('/undo restored and removed', open(src).read() == 'x = 1\n' and not os.path.exists(new) and 'restored a.py' in s and 'removed b.py' in s)
os.write(fd, b'/undo\r'); pump(0.5)
ok('second /undo has nothing to do', 'nothing to undo' in out.decode(errors='replace'))
os.write(fd, b'next\r'); pump(2)
ok('model told about the undo', 'undid the file changes' in seen[-1])
os.write(fd, b'\x04'); pump(1)
