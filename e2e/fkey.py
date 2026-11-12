"""Enter during a turn: at a tool boundary the model gets it; otherwise it is the next prompt."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
seen, n = [], [0]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        seen.append(json.dumps(body['messages']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        n[0] += 1
        def ev(d): self.wfile.write(f'data: {json.dumps(d)}\n\n'.encode()); self.wfile.flush()
        last = json.dumps(body['messages'][-1])
        if 'run it' in last:
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Bash', 'arguments': json.dumps({'command': 'sleep 1.5; echo done'})}}]}, 'finish_reason': 'tool_calls'}]})
        elif 'slow reply' in last:
            for w in ['thinking ', 'about ', 'it ']:
                ev({'choices': [{'delta': {'content': w}}]}); time.sleep(0.5)
            ev({'choices': [{'delta': {}, 'finish_reason': 'stop'}]})
        else:
            ev({'choices': [{'delta': {'content': 'ok'}, 'finish_reason': 'stop'}]})
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
pump(1); os.write(fd, b'run it\r'); pump(0.6)
os.write(fd, b'use printf\r'); pump(3)
s = out.decode(errors='replace')
ok('queued notice shown after the command output', 'queued: use printf' in s and s.find('done') < s.find('queued: use printf'))
ok('model got it with the tool results', any('while you were working:]\\nuse printf' in m for m in seen))
os.write(fd, b'slow reply\r'); pump(0.4); os.write(fd, b'\x1b[15~\x1b[Z'); pump(2.5); ok('F5 and Shift-Tab do not stop a turn', 'thinking about it' in out.decode(errors='replace') and 'stopped' not in out.decode(errors='replace')); os.write(fd, b'and then?\r'); pump(1)
ok('leftover became the next prompt', any(m.endswith('"and then?"}]}]') or '"and then?"' in m for m in seen[-1:]))
os.write(fd, b'\x04'); pump(0.5)
