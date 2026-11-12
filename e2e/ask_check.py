"""The guard plugin's `ask` decision, answered in a pseudo-terminal."""
import http.server, json, os, pty, select, sys, threading, time

BIN, HOME, PLUGINS = sys.argv[1], sys.argv[2], sys.argv[3]
requests = []


class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'

    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        requests.append(body)
        last = body['messages'][-1]
        if last['role'] == 'user':
            delta = {'tool_calls': [{'index': 0, 'id': 'c1', 'function': {
                'name': 'Bash', 'arguments': json.dumps({'command': 'git push origin topic'})}}]}
            finish = 'tool_calls'
        else:
            delta, finish = {'content': 'ok'}, 'stop'
        self.send_response(200)
        self.send_header('content-type', 'text/event-stream')
        self.end_headers()
        for chunk in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': finish}]}]:
            self.wfile.write(f'data: {json.dumps(chunk)}\n\n'.encode())
        self.wfile.write(b'data: [DONE]\n\n')


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=server.serve_forever, daemon=True).start()
os.makedirs(os.path.join(HOME, 'plugins'), exist_ok=True)
os.symlink(os.path.join(PLUGINS, 'guard'), os.path.join(HOME, 'plugins', 'guard'))
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', NO_COLOR='1',
           STUB_BASE_URL=f'http://127.0.0.1:{server.server_port}', TERM='xterm')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(HOME)
    os.execve(BIN, [BIN, '--no-session'], env)
out = b''


def read_until(text, timeout=5.0):
    global out
    start, end = len(out), time.time() + timeout
    while time.time() < end:
        if text.encode() in out[start:]:
            return True
        if select.select([fd], [], [], 0.05)[0]:
            try:
                out += os.read(fd, 4096)
            except OSError:
                break
    return text.encode() in out[start:]


ok = read_until('› ')
os.write(fd, b'push my branch\r')
asked = read_until('Allow Bash git push origin topic (pushes to a remote)? [y/N]')
os.write(fd, b'n')
answered = read_until('context ')
result = [m for m in requests[-1]['messages'] if m['role'] == 'tool'][-1]['content'] if len(requests) > 1 else ''
read_until('› ', 3)
os.write(fd, b'\x04')
exited = False
# A closed terminal makes read_until return at once, so wait by the clock.
deadline = time.time() + 6
while time.time() < deadline:
    done, status = os.waitpid(pid, os.WNOHANG)
    if done:
        exited = True
        break
    read_until('\x00', 0.1)
    time.sleep(0.02)
if not exited:
    print('STILL RUNNING; tail:', repr(out.decode(errors='replace')[-300:]))
    os.kill(pid, 9)
checks = {'prompt': ok, 'guard asked': asked, 'turn finished': answered, 'exits on Ctrl-D': exited,
          'model told it was declined': result == 'the user declined this call (pushes to a remote)'}
for k, v in checks.items():
    print(('PASS ' if v else 'FAIL ') + k)
if not all(checks.values()):
    print(repr(out.decode(errors='replace')[-600:]), repr(result))
    sys.exit(1)
