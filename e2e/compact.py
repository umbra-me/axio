"""Automatic compaction: a big context triggers a summary, then the turn goes on."""
import re, http.server, json, os, pty, select, sys, threading, time
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
seen = []
class Stub(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['content-length'])))
        seen.append(body)
        self.send_response(200); self.send_header('content-type', 'text/event-stream'); self.end_headers()
        last = json.dumps(body['messages'][-1])
        if 'Summarize the transcript' in last:
            text, used = '<summary>The user asked about apples and pears.</summary>', 900
        else:
            text, used = f'answer {len(seen)}', 5000
        self.wfile.write(f'data: {json.dumps({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]})}\n\n'.encode())
        self.wfile.write(f'data: {json.dumps({"choices": [], "usage": {"prompt_tokens": used, "completion_tokens": 5}})}\n\ndata: [DONE]\n\n'.encode())
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/test', AXIO_COMPACT_AT='4000', STUB_BASE_URL=f'http://127.0.0.1:{srv.server_port}', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0: os.chdir(HOME); os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
pump(1)
for p in [b'tell me about apples\r', b'and pears?\r', b'which was first?\r']:
    os.write(fd, p); pump(1.5)
s = out.decode(errors='replace'); print(re.findall(r'compacted: [^\r\n]*', s))

summaries = [b for b in seen if 'Summarize the transcript' in json.dumps(b['messages'][-1])]
ok('compacted once the context passed the limit', len(summaries) >= 1 and 'compacting the conversation' in s and re.search(r'compacted: 5.0k → about [0-9.]+k? tokens', s) is not None)
after = json.dumps(seen[-1]['messages'])
ok('later requests carry the summary, not the old turns', 'apples and pears' in after and 'tell me about apples' in after)
ok('turn after compaction answered', s.count('answer ') >= 3)
os.write(fd, b'\x04'); pump(0.5)
