"""The sandbox module on Linux: Bash and Write limits under -p full."""
import http.server, json, os, pty, select, sys, threading, time
BIN, HOME, PROJ = sys.argv[1], sys.argv[2], sys.argv[3]
outside = os.path.expanduser('~/outside.txt')
calls = [
    ('Bash', {'command': 'echo hi > inside.txt && cat inside.txt'}),
    ('Bash', {'command': f'echo x > {outside}; echo rc=$?'}),
    ('Bash', {'command': 'echo t > /tmp/sbx.txt && echo tmp-ok'}),
    ('Bash', {'command': '(exec 3<>/dev/tcp/1.1.1.1/80) 2>&1; echo net-rc=$?'}),
    ('Write', {'file_path': os.path.expanduser('~/outside2.txt'), 'content': 'x\n'}),
]
results = []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        for m in body['messages']:
            if m['role'] == 'tool' and m not in results: results.append(m)
        i = sum(1 for m in body['messages'] if m['role'] == 'assistant')
        if i < len(calls):
            name, args = calls[i]
            delta = {'tool_calls': [{'index': 0, 'id': f'c{i}', 'function': {'name': name, 'arguments': json.dumps(args)}}]}; fin = 'tool_calls'
        else:
            delta, fin = {'content': 'done'}, 'stop'
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        for c in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': fin}]}]:
            self.wfile.write(f'data: {json.dumps(c)}\n\n'.encode())
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', NO_COLOR='1', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', TERM='xterm', AXIO_PERMISSION_MODE='bypassPermissions')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(PROJ); os.execve(BIN, [BIN, '--no-session', '--trust', '--profile', 'full'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            try: out += os.read(fd, 65536)
            except OSError: return
pump(1.5); os.write(fd, b'go\r'); pump(6); os.write(fd, b'\x04'); pump(1)
text = [str(m.get('content')) for m in results]
for t in text: print('  result:', t[:160].replace('\n', ' | '))
def ok(name, cond): print(('PASS ' if cond else 'FAIL ') + name)
ok('writes inside the project', os.path.exists(os.path.join(PROJ, 'inside.txt')) and 'hi' in text[0])
ok('bash cannot write outside', not os.path.exists(outside) and 'rc=1' in text[1])
ok('bash can write /tmp', 'tmp-ok' in text[2])
ok('no network by default', 'net-rc=0' not in text[3] and 'net-rc=' in text[3])
ok('Write refused outside', not os.path.exists(os.path.expanduser('~/outside2.txt')))
