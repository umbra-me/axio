"""A slowly streamed Write call: the spinner should count lines; then /fast and /effort."""
import http.server, json, os, pty, re, select, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
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
            args = json.dumps({'file_path': os.path.join(HOME, 'out.s'), 'content': ''.join(f'line {i}\n' for i in range(40))})
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {'name': 'Write', 'arguments': ''}}]}}]})
            for i in range(0, len(args), 12):
                ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'function': {'arguments': args[i:i+12]}}]}}]}); time.sleep(0.05)
            ev({'choices': [{'delta': {}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'ok'}, 'finish_reason': 'stop'}]})
        ev({'choices': [], 'usage': {'prompt_tokens': 10, 'completion_tokens': 5}})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', TERM='xterm-256color')
env.pop('NO_COLOR', None)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(HOME); os.execve(BIN, [BIN, '--no-session', '--trust'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
pump(1); os.write(fd, b'go\r'); pump(5)
os.write(fd, b'/fast\r'); pump(0.5); os.write(fd, b'/effort low\r'); pump(0.5); os.write(fd, b'/model\r'); pump(0.5)
os.write(fd, b'\x04'); pump(1)
s = out.decode(errors='replace')
labels = re.findall(r'\x1b\[2m(writing [^\x1b]*)', s)
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
ok('label names the file', any('out.s' in l for l in labels))
counts = [int(m) for l in labels for m in re.findall(r'· (\d+) lines', l)]
ok(f'line count climbs ({counts[:3]}…{counts[-2:]})', len(counts) > 5 and counts == sorted(counts) and counts[-1] >= 40)
ok('fast toggled', 'fast mode on for this session' in s)
ok('effort set', 'effort: low for this session' in s)
ok('/model shows both', 'test low fast' in s)
ok('file written', os.path.exists(os.path.join(HOME, 'out.s')))
