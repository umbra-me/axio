import http.server, json, os, subprocess, sys, threading, time
BIN, HOME = sys.argv[1], sys.argv[2]
script = [
    ('TodoWrite', {'todos': [{'content': 'find the bug', 'status': 'in_progress'}, {'content': 'fix it', 'status': 'pending'}]}),
    ('Grep', {'pattern': 'needle', 'output_mode': 'content'}),
    ('Glob', {'pattern': '**/*.txt'}),
    ('Bash', {'command': 'sleep 0.3; echo built', 'run_in_background': True}),
    ('Bash', {'command': 'sleep 1'}),
]
results, tools_offered, notes = [], [], []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        tools_offered.append([t['function']['name'] for t in body['tools']])
        msgs = body['messages']
        if msgs[-1]['role'] == 'tool': results.append(msgs[-1]['content'])
        notes.extend(m['content'] for m in msgs if m['role'] == 'user' and 'Background task' in (m['content'] or ''))
        step = len(results)
        if step < len(script):
            name, args = script[step]
            delta = {'tool_calls': [{'index': 0, 'id': f'c{step}', 'function': {'name': name, 'arguments': json.dumps(args)}}]}
            fin = 'tool_calls'
        else:
            delta, fin = {'content': 'done'}, 'stop'
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        for c in [{'choices': [{'delta': delta}]}, {'choices': [{'delta': {}, 'finish_reason': fin}]}]:
            self.wfile.write(f'data: {json.dumps(c)}\n\n'.encode())
        self.wfile.write(b'data: [DONE]\n\n')
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=server.serve_forever, daemon=True).start()
os.makedirs(os.path.join(HOME, 'proj/docs'), exist_ok=True)
open(os.path.join(HOME, 'proj/docs/a.txt'), 'w').write('a needle here\n')
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', STUB_BASE_URL=f'http://127.0.0.1:{server.server_port}')
p = subprocess.run([BIN, '-p', '--no-session', 'go'], env=env, cwd=os.path.join(HOME, 'proj'), capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)
print('exit', p.returncode)
print('tools offered:', tools_offered[0])
for name_args, r in zip(script, results): print(f'{name_args[0]}:', repr(r[:110]))
print('background note reached the model:', any('bg1' in n and 'exited with code 0' in n for n in notes))
q = subprocess.run([BIN, '-p', '--no-session', '--profile', 'minimal', 'go'], env=env, cwd=os.path.join(HOME, 'proj'), capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)
print('minimal tools:', tools_offered[-1])
