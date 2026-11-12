"""Rewind (Esc twice) with files, resending the rewound message, and /history."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
work = os.path.join(HOME, 'work'); os.makedirs(work, exist_ok=True)
note = os.path.join(work, 'note.txt')
seen = []
def sse(delta, fin):
    return ''.join(f'data: {json.dumps(c)}\n\n' for c in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': fin}]}]) + 'data: [DONE]\n\n'
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        msgs = [m for m in body['messages'] if m['role'] != 'system']
        seen.append(msgs)
        last = msgs[-1]
        if last['role'] == 'user' and 'make a file' in json.dumps(last):
            out = sse({'tool_calls': [{'index': 0, 'id': 'w', 'function': {'name': 'Write', 'arguments': json.dumps({'file_path': note, 'content': 'hi'})}}]}, 'tool_calls')
        else:
            out = sse({'content': 'ok'}, 'stop')
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        self.wfile.write(out.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/m', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}/v1',
           STUB_API_KEY='k', NO_COLOR='1', TERM='xterm', COLUMNS='120')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(work); os.execve(BIN, [BIN], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
def step(keys, t=1.0, gap=0.3):
    start = len(out)
    for k in keys:
        os.write(fd, k); pump(gap)
    pump(t)
    return out[start:].decode(errors='replace')
pump(2)
step([b'make a file\r'], 2)
wrote = os.path.exists(note)
step([b'second message\r'], 2)
picker = step([b'\x1b', b'\x1b'], 1, gap=0.1)
actions = step([b'\x1b[B', b'\r'], 1)
rewound = step([b'\x1b[B', b'\r'], 1)
removed = not os.path.exists(note)
before = len(seen)
step([b'\r'], 2)
resent = seen[before] if len(seen) > before else []
hist = step([b'/history\r'], 1)
picked = step([b'\r'], 1)
os.write(fd, b'\x04'); pump(0.5)
users = [m for m in resent if m['role'] == 'user']
checks = [
    ('the first turn wrote the file', wrote),
    ('Esc twice lists your messages, newest first', 'Rewind to before' in picker and picker.find('second message') < picker.find('make a file')),
    ('a file changed since is offered back', 'The conversation and files' in actions and 'note.txt' in actions),
    ('rewinding with files removes it', removed and 'removed note.txt' in rewound),
    ('the message comes back to edit', '› make a file' in rewound),
    ('resending starts from that point', len(users) == 1 and 'make a file' in json.dumps(users[0])),
    ('/history lists past prompts', 'Prompt history' in hist and 'second message' in hist),
    ('choosing one puts it in the prompt', '› ' in picked),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(out.decode(errors='replace')[-3000:])
