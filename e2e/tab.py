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
pump(1); os.write(fd, b'/he\t'); pump(0.3); os.write(fd, b'\r'); pump(0.8); os.write(fd, b'\x04'); pump(0.5)
s = out.decode(errors='replace')
print('PASS tab completed /help' if '/help ' in s and 'pick or set reasoning effort' in s else 'FAIL tab\n' + s[-600:])
