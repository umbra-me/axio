"""/context, /settings and /init in a terminal."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
asked = []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        asked.append(json.dumps(body['messages'][-1]))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        for c in [{'choices': [{'delta': {'content': 'done'}}]}, {'choices': [{'delta': {}, 'finish_reason': 'stop'}]}]:
            self.wfile.write(f'data: {json.dumps(c)}\n\n'.encode())
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
work = os.path.join(HOME, 'work')
os.makedirs(os.path.join(work, '.git'), exist_ok=True)
open(os.path.join(work, 'AGENTS.md'), 'w').write('Run make test.\n' * 50)
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/m', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}/v1',
           STUB_API_KEY='k', NO_COLOR='1', TERM='xterm', COLUMNS='120')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
def step(keys, t=1.0):
    start = len(out)
    for k in keys:
        os.write(fd, k); pump(0.3)
    pump(t)
    return out[start:].decode(errors='replace')
pump(2)
context = step([b'/context\r'])
fast = step([b'/settings\r', b'fast mode by', b'\r'])
parallel = step([b'/settings\r', b'at once', b'\r', b'2', b'\r'])
toggled = step([b'/settings\r', b'under the', b'\r', b'cost', b'\r', b'\x1b'], 1)
after_toggle = step([b'x', b'\x15'], 0.5)
step([b'/init\r'], 2.5)
os.write(fd, b'\x04'); pump(0.5)
saved = json.load(open(os.path.join(HOME, 'settings.json')))
checks = [
    ('/context draws the window', 'tokens (' in context and 'Free space' in context and 'compaction starts at' in context),
    ('/context names its parts', 'System prompt' in context and 'Built-in tools' in context and 'AGENTS.md' in context),
    ('/settings turns fast mode on for new sessions', saved.get('fastMode') is True and 'Fast mode by default: on for new sessions' in fast),
    ('/settings sets a nested value', saved.get('subagents', {}).get('parallel') == 2 and 'Sub-agents at once: 2' in parallel),
    ('a status line item can be switched off', saved.get('statusLine', {}).get('underPrompt') == ['model', 'context'] and 'Under the prompt: model, context (now and for new sessions)' in toggled),
    ('and the status line drops it at once', 'no price list' not in after_toggle and '% context' in after_toggle),
    ('/init asks for AGENTS.md at the project root', any('AGENTS.md at ' + os.path.realpath(work) in a or 'AGENTS.md at ' + work in a for a in asked)),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(saved, asked[-1:] if asked else None); print(out.decode(errors='replace')[-2500:])
