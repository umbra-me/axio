import os, pty, select, sys, time
BIN, HOME = sys.argv[1], sys.argv[2]
env = dict(os.environ, AXIO_HOME=HOME, AXIO_MODEL='stub/x', STUB_BASE_URL='http://127.0.0.1:9', NO_COLOR='1', TERM='xterm')
pid, fd = pty.fork()
if pid == 0: os.execve(BIN, [BIN, '--no-session'], env)
out = b''
def pump(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 65536)
            except OSError: return
def send(b, t=0.6):
    os.write(fd, b); pump(t)
pump(1)
send(b'/ef'); menu = out.decode(errors='replace')
send(b'\r'); picker = out.decode(errors='replace')
send(b'\x1b[D\t'); effort = out.decode(errors='replace')
send(b'/model\r', 2.5); listing = out.decode(errors='replace')
send(b'y\r'); model = out.decode(errors='replace')
send(b'\x04', 0.5)
checks = [
    ('slash menu lists /effort as you type', '/effort' in menu and 'pick or set reasoning effort' in menu),
    ('Enter opens the effort slider', 'Faster' in picker and 'Smarter' in picker),
    ('the slider sets effort for this session', 'effort: medium (this session)' in effort),
    ('an unlisted provider says why', 'stub: could not list models' in listing),
    ('a typed model id is used', 'model: stub/y' in model),
]
for name, ok in checks:
    print(('PASS ' if ok else 'FAIL ') + name)
if not all(ok for _, ok in checks):
    print(out.decode(errors='replace')[-1500:])
