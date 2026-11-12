"""Sub-agents in the terminal: the progress line, finished lines, and Esc stopping them all."""
import http.server, json, os, pty, select, subprocess, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
mode = {'slow': 1.0}

def sse(delta, fin):
    return ''.join(f'data: {json.dumps(c)}\n\n' for c in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': fin}]}]) + 'data: [DONE]\n\n'

class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        child = body['messages'][0]['content'].startswith('You are a sub-agent')
        if child:
            n = sum(1 for m in body['messages'] if m['role'] == 'assistant')
            if n == 0:
                out = sse({'tool_calls': [{'index': 0, 'id': 'g', 'function': {'name': 'Glob', 'arguments': json.dumps({'pattern': '*.txt'})}}]}, 'tool_calls')
            else:
                time.sleep(mode['slow'])
                out = sse({'content': 'child answer'}, 'stop')
        elif any(m['role'] == 'tool' for m in body['messages']):
            out = sse({'content': 'all done'}, 'stop')
        else:
            calls = [{'index': i, 'id': f'c{i}', 'function': {'name': 'Agent', 'arguments': json.dumps(
                {'description': f'look {w}', 'prompt': f'find {w}', 'subagent_type': 'explore'})}} for i, w in enumerate('AB')]
            out = sse({'tool_calls': calls}, 'tool_calls')
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        try: self.wfile.write(out.encode())
        except OSError: pass

srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
os.makedirs(HOME + '/proj', exist_ok=True)
open(HOME + '/auth.json', 'w').write(json.dumps({'stub': {'baseUrl': f'http://127.0.0.1:{srv.server_port}/v1', 'apiKey': 'k'}}))
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/m', TERM='xterm', COLUMNS='120')
env.pop('NO_COLOR', None)
results = {}

def session(keys_after_spin):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(HOME + '/proj'); os.execve(BIN, [BIN, '--no-session'], env)
    out = b''
    def until(text, t=8.0):
        nonlocal out
        end = time.time() + t
        while time.time() < end:
            if text.encode() in out: return True
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try: out += os.read(fd, 65536)
                except OSError: return text.encode() in out
        return text.encode() in out
    until('› ')
    os.write(fd, b'go\r')
    spun = until('2 agents', 6)
    t0 = time.time()
    if keys_after_spin:
        os.write(fd, keys_after_spin)
    back = until('all done', 10) if not keys_after_spin else until('stopped', 6)
    took = time.time() - t0
    until('› ', 4)
    kids = subprocess.run(['pgrep', '-f', f'{BIN} agent --attached'], capture_output=True, text=True).stdout.split()
    os.write(fd, b'\x04')
    time.sleep(0.5)
    try: os.kill(pid, 9)
    except OSError: pass
    return spun, back, took, kids, out.decode(errors='replace')

spun, done, took, kids, screen = session(None)
results['progress line shows both agents'] = spun
results['both finish and the main agent answers'] = done and 'all done' in screen
results['finished lines'] = '✓' in screen and 'explore · look A' in screen and 'explore · look B' in screen
mode['slow'] = 30
spun, stopped, took, kids, screen = session(b'\x1b')
results['Esc stops the turn within 3s'] = stopped and took < 3
results['no sub-agent processes left'] = kids == []
for k, v in results.items():
    print(('PASS ' if v else 'FAIL ') + k)
if not all(results.values()):
    print(repr(screen[-1500:]))
