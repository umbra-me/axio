"""Permission modes: asking, remembering for the session, Shift+Tab to accept edits and plan mode, and a sub-agent's question."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
work = os.path.join(HOME, 'work'); os.makedirs(work, exist_ok=True)
ran, edited, sub = (os.path.join(work, n) for n in ('ran.txt', 'edited.txt', 'sub.txt'))
def sse(delta, fin):
    return ''.join(f'data: {json.dumps(c)}\n\n' for c in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': fin}]}]) + 'data: [DONE]\n\n'
def call(name, args):
    return sse({'tool_calls': [{'index': 0, 'id': 'c', 'function': {'name': name, 'arguments': json.dumps(args)}}]}, 'tool_calls')
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        msgs = body['messages']; last = msgs[-1]
        child = msgs[0]['content'].startswith('You are a sub-agent')
        if last['role'] == 'tool':
            out = sse({'content': 'ok'}, 'stop')
        elif child:
            out = call('Bash', {'command': f'mkdir {sub}'})
        else:
            said = json.dumps(last)
            if 'delegate' in said:
                out = call('Agent', {'description': 'touch', 'prompt': 'touch it', 'subagent_type': 'runner'})
            elif 'edit' in said:
                out = call('Write', {'file_path': edited, 'content': 'x'})
            else:
                out = call('Bash', {'command': f'touch {ran}'})
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(out.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
os.makedirs(os.path.join(HOME, 'agents'), exist_ok=True)
open(os.path.join(HOME, 'agents', 'runner.md'), 'w').write('---\nname: runner\ndescription: Runs things\ntools: Bash\n---\nRun it.\n')
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/m', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}/v1',
           STUB_API_KEY='k', NO_COLOR='1', TERM='xterm', COLUMNS='140')
env.pop('AXIO_PERMISSION_MODE', None)
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
        os.write(fd, k); pump(0.4)
    pump(t)
    return out[start:].decode(errors='replace')
down = b'\x1b[B'
pump(2)
asked = step([b'run\r'], 1.5)
step([down, down, down, b'\r'], 1.5)        # No
declined = not os.path.exists(ran)
step([b'run\r'], 1.5)
step([down, down, b'\r'], 1.5)              # Yes, for the rest of this session
session_yes = os.path.exists(ran)
os.remove(ran)
again = step([b'run\r'], 2)
no_second_ask = os.path.exists(ran) and 'Run `touch' not in again
edits = step([b'\x1b[Z'], 0.5)
step([b'edit\r'], 2)
plan = step([b'\x1b[Z'], 0.5)
os.remove(ran)
planned = step([b'run\r'], 2)
step([b'\x1b[Z'], 0.5)                      # back to default
delegated = step([b'delegate\r'], 3)
step([b'\r'], 3)                            # Yes, for the sub-agent's command
os.write(fd, b'\x04'); pump(0.5)
checks = [
    ('default mode asks before a command', 'Run `touch' in asked and "Yes, and don't ask again for Bash(touch:*) in this project" in asked),
    ('No means it does not run', declined),
    ('yes for the session runs it', session_yes),
    ('and does not ask again', no_second_ask),
    ('Shift+Tab turns on accept edits', 'accept edits (shift+tab)' in edits),
    ('accepted edits run without asking', os.path.exists(edited)),
    ('Shift+Tab again is plan mode', 'plan mode (shift+tab)' in plan),
    ('plan mode refuses commands', not os.path.exists(ran) and 'plan mode' in planned),
    ("a sub-agent's question reaches the terminal", 'runner · touch: Run `mkdir' in delegated),
    ('with the same choices as the main agent', "don't ask again for Bash(mkdir:*)" in delegated),
    ('and the answer lets it run', os.path.exists(sub)),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(out.decode(errors='replace')[-3000:])
