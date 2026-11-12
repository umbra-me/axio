"""Drive the axio binary in a pseudo-terminal against a local stub provider."""
import http.server
import json
import os
import pty
import select
import sys
import threading
import time

BIN, HOME = sys.argv[1], sys.argv[2]
requests = []


class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'

    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        requests.append(body)
        last = body['messages'][-1]['content'] or ''
        self.send_response(200)
        self.send_header('content-type', 'text/event-stream')
        self.end_headers()
        words = ['tick '] * 40 if last.endswith('slow') else ['hello ', 'there']
        try:
            for w in words:
                chunk = {'choices': [{'delta': {'content': w}}]}
                self.wfile.write(f'data: {json.dumps(chunk)}\n\n'.encode())
                self.wfile.flush()
                time.sleep(0.15 if last.endswith('slow') else 0)
            done = {'choices': [{'delta': {}, 'finish_reason': 'stop'}],
                    'usage': {'prompt_tokens': 1200, 'completion_tokens': 3}}
            self.wfile.write(f'data: {json.dumps(done)}\n\ndata: [DONE]\n\n'.encode())
        except BrokenPipeError:
            requests.append('client closed')


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=server.serve_forever, daemon=True).start()

env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', NO_COLOR='1',
           STUB_BASE_URL=f'http://127.0.0.1:{server.server_port}', TERM='xterm', COLUMNS='80')
pid, fd = pty.fork()
if pid == 0:
    os.execve('/bin/sh', ['sh', '-c', f'stty -g > {HOME}/before; "{BIN}" --no-session; stty -g > {HOME}/after'], env)

out = b''


def read_until(text, timeout=5.0):
    global out
    start = len(out)
    end = time.time() + timeout
    while time.time() < end:
        if text.encode() in out[start:]:
            return True
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                out += os.read(fd, 4096)
            except OSError:
                break
    return text.encode() in out[start:]


def send(data):
    os.write(fd, data.encode() if isinstance(data, str) else data)


class Checks(dict):
    def __setitem__(self, k, v):
        print(('PASS ' if v else 'FAIL ') + k, flush=True)
        super().__setitem__(k, v)


checks = Checks()
checks['prompt shown'] = read_until('› ')
send('hi\r')
checks['answer streamed'] = read_until('hello there')
checks['status line'] = read_until('1.2k of 128.0k context') or b'1.2k of 128.0k context' in out
mark = len(out)
send('slow\r')
checks['slow started'] = read_until('tick')
t0 = time.time()
send('\x1b')
checks['esc stops the turn'] = read_until('stopped', 2.0)
checks['stopped quickly'] = time.time() - t0 < 1.0
time.sleep(1.5)
checks['provider saw the disconnect'] = 'client closed' in requests
send('again\r')
checks['next turn ran'] = read_until('hello there', 5.0)
if not checks['next turn ran']:
    read_until('zzz', 3)
    print(repr(out.decode(errors='replace')[-400:]))
    os.kill(pid, 9)
    sys.exit(1)
last_user = [m for m in requests[-1]['messages'] if m['role'] == 'user'][-1]['content']
checks['aborted prompt folded into the next'] = last_user.startswith('slow\n\n[The user interrupted') and last_user.endswith('\n\nagain')
send('\x1b[A')
checks['history recalls the last prompt'] = read_until('› again', 2.0)
send('\x15')  # Ctrl-U clears
send('\x04')  # Ctrl-D exits
end = time.time() + 5
status = None
while time.time() < end:
    done, status = os.waitpid(pid, os.WNOHANG)
    if done:
        break
    read_until('\x00', 0.1)
    status = None
if status is None:
    os.kill(pid, 9)
    print(out.decode(errors='replace')[-800:])
    status = 9 << 8
checks['exits cleanly on Ctrl-D'] = os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
history = open(os.path.join(HOME, 'history.jsonl')).read().splitlines()
checks['terminal settings restored'] = open(os.path.join(HOME, 'before')).read() == open(os.path.join(HOME, 'after')).read()
checks['history saved'] = [json.loads(h) for h in history] == ['hi', 'slow', 'again']

if not all(checks.values()):
    print(out.decode(errors='replace')[-1500:])
    sys.exit(1)
