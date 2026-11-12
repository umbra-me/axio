"""First interactive run without a key: prompt, save to auth.json, then /help."""
import json, os, pty, select, stat, sys, time
BIN, HOME = sys.argv[1], sys.argv[2]
env = {k: v for k, v in os.environ.items() if not k.startswith(('ANTHROPIC_', 'AXIO_'))}
env.update(AXIO_HOME=HOME, AXIO_MODEL='anthropic/claude-opus-5-5', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0:
    os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def until(t, timeout=5):
    global out
    start = len(out); end = time.time() + timeout
    while time.time() < end:
        if t.encode() in out[start:]: return True
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 4096)
            except OSError: break
    return t.encode() in out[start:]
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + name)
ok('asks for a key', until('API key: '))
os.write(fd, b'sk-test-0000\r')
ok('saved', until('saved;'))
ok('key not echoed', b'sk-test-0000' not in out)
p = os.path.join(HOME, 'auth.json')
ok('auth.json holds it', json.load(open(p))['anthropic']['apiKey'] == 'sk-test-0000')
ok('auth.json is 0600', stat.S_IMODE(os.stat(p).st_mode) == 0o600)
ok('prompt shown', until('› '))
os.write(fd, b'/help\r')
ok('help listed', until('Esc stops a turn'))
os.write(fd, b'\x04'); until('zzz', 1)
print(out.decode(errors='replace').split('/help')[-1].replace('\r', ''))
