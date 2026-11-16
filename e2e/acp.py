"""axio acp: a client on stdin/stdout against a stub model. Every stdout line
must be JSON-RPC; a Bash call asks permission; closing stdin ends the process."""
import http.server, json, os, subprocess, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
n = [0]
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['content-length']))
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        n[0] += 1
        def ev(d): self.wfile.write(f'data: {json.dumps(d)}\n\n'.encode()); self.wfile.flush()
        calls = [('Bash', {'command': 'echo acp-ran'}), ('Write', {'file_path': 'made.txt', 'content': 'hi\n'}),
                 ('Edit', {'file_path': 'made.txt', 'old_string': 'hi', 'new_string': 'hello'}),
                 ('TodoWrite', {'todos': [{'content': 'check', 'status': 'in_progress', 'activeForm': 'checking'}]})]
        if n[0] <= len(calls):
            name, args = calls[n[0] - 1]
            ev({'choices': [{'delta': {'tool_calls': [{'index': 0, 'id': f'c{n[0]}', 'function': {'name': name, 'arguments': json.dumps(args)}}]}, 'finish_reason': 'tool_calls'}]})
        else:
            ev({'choices': [{'delta': {'content': 'all done'}, 'finish_reason': 'stop'}]})
        self.wfile.write(b'data: [DONE]\n\n')
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', AXIO_PERMISSION_MODE='default',
           STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}')
p = subprocess.Popen([BIN, 'acp'], cwd=HOME, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL)
lines, bad = [], []
def read():
    for raw in p.stdout:
        try: lines.append(json.loads(raw))
        except ValueError: bad.append(raw)
threading.Thread(target=read, daemon=True).start()
sent = {}
def send(m):
    if 'method' in m and 'id' in m: sent[m['id']] = m['method']
    p.stdin.write((json.dumps(m) + '\n').encode()); p.stdin.flush()
def wait(f, t=10):
    end = time.time() + t
    while time.time() < end:
        for v in list(lines):
            if f(v): return v
        time.sleep(0.02)
    return None
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + 'acp ' + name)
send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': 1, 'clientCapabilities': {}}})
r = wait(lambda v: v.get('id') == 1)
ok('initialize', r and r['result']['protocolVersion'] == 1)
send({'jsonrpc': '2.0', 'id': 2, 'method': 'session/new', 'params': {'cwd': HOME, 'mcpServers': []}})
r = wait(lambda v: v.get('id') == 2 and 'method' not in v)
sid = r and r.get('result', {}).get('sessionId')
ok('session/new', bool(sid))
send({'jsonrpc': '2.0', 'id': 3, 'method': 'session/prompt', 'params': {'sessionId': sid, 'prompt': [{'type': 'text', 'text': 'run it'}]}})
ask = wait(lambda v: v.get('method') == 'session/request_permission')
ok('Bash asks', ask is not None)
def yes():
    done = set()
    while p.poll() is None:
        for v in list(lines):
            if v.get('method') == 'session/request_permission' and v['id'] not in done:
                done.add(v['id'])
                send({'jsonrpc': '2.0', 'id': v['id'], 'result': {'outcome': {'outcome': 'selected', 'optionId': '0'}}})
        time.sleep(0.02)
threading.Thread(target=yes, daemon=True).start()
r = wait(lambda v: v.get('id') == 3 and 'method' not in v, 20)
ok('prompt ends the turn', r and r.get('result', {}).get('stopReason') == 'end_turn')
ups = [v['params']['update'] for v in lines if v.get('method') == 'session/update']
ran = [u for u in ups if u.get('sessionUpdate') == 'tool_call_update' and u.get('status') == 'completed']
ok('tool output reaches the client', any('acp-ran' in json.dumps(u) for u in ran))
ok('Write and Edit show diffs', sum(1 for u in ups if any(c.get('type') == 'diff' for c in (u.get('content') if isinstance(u.get('content'), list) else []))) >= 2)
ok('Edit changed the file', open(os.path.join(HOME, 'made.txt')).read() == 'hello\n')
ok('TodoWrite becomes a plan', any(u.get('sessionUpdate') == 'plan' and u['entries'][0]['status'] == 'in_progress' for u in ups))
ok('answer streams', any(u.get('sessionUpdate') == 'agent_message_chunk' and u['content']['text'] == 'all done' for u in ups))
send({'jsonrpc': '2.0', 'id': 4, 'method': 'session/set_config_option', 'params': {'sessionId': sid, 'configId': 'mode', 'value': 'plan'}})
r = wait(lambda v: v.get('id') == 4 and 'method' not in v)
ok('config option', r and r['result']['configOptions'][-1]['currentValue'] == 'plan')
send({'jsonrpc': '2.0', 'id': 5, 'method': 'session/load', 'params': {'sessionId': sid, 'cwd': HOME, 'mcpServers': []}})
r = wait(lambda v: v.get('id') == 5 and 'method' not in v)
ok('session/load', r is not None and 'result' in r)
send({'jsonrpc': '2.0', 'id': 6, 'method': 'session/list', 'params': {'cwd': HOME}})
r = wait(lambda v: v.get('id') == 6 and 'method' not in v)
ok('session/list', r and r['result']['sessions'][0]['sessionId'] == sid and r['result']['sessions'][0]['title'] == 'run it')
ok('stdout is only JSON', not bad)
if os.environ.get('ACP_DUMP'):
    json.dump({'sent': sent, 'lines': lines}, open(os.environ['ACP_DUMP'], 'w'))
p.stdin.close()
try: p.wait(5); ok('exits when stdin closes', True)
except subprocess.TimeoutExpired: p.kill(); ok('exits when stdin closes', False)
