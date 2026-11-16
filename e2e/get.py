"""scripts/get.sh installs a release binary only when SHA256SUMS agrees.
Serves a fake release from a local folder; nothing is downloaded."""
import functools, hashlib, http.server, os, platform, shutil, subprocess, sys, threading
BIN, HOME = sys.argv[1], os.path.abspath(sys.argv[2])
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
m = platform.machine()
arch = 'aarch64' if m in ('arm64', 'aarch64') else 'x86_64'
target = f'{arch}-apple-darwin' if sys.platform == 'darwin' else f'{arch}-unknown-linux-musl'
rel = os.path.join(HOME, 'release'); os.makedirs(rel)
shutil.copy(BIN, os.path.join(rel, f'axio-{target}'))
digest = hashlib.sha256(open(BIN, 'rb').read()).hexdigest()
def sums(d): open(os.path.join(rel, 'SHA256SUMS'), 'w').write(f'{d}  axio-{target}\n')
class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
handler = functools.partial(Quiet, directory=rel)
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
threading.Thread(target=srv.serve_forever, daemon=True).start()
def run(dest):
    env = dict(os.environ, AXIO_DOWNLOAD_URL=f'http://127.0.0.1:{srv.server_port}', AXIO_INSTALL_DIR=dest)
    return subprocess.run(['sh', os.path.join(ROOT, 'scripts/get.sh')], env=env, capture_output=True, text=True)
ok = lambda name, v: print(('PASS ' if v else 'FAIL ') + 'get.sh ' + name)
sums('0' * 64)
bad = os.path.join(HOME, 'bad')
r = run(bad)
ok('refuses a wrong checksum', r.returncode != 0 and 'does not match' in r.stderr and not os.path.exists(os.path.join(bad, 'axio')))
sums(digest)
good = os.path.join(HOME, 'good')
r = run(good)
ok('installs a matching binary', r.returncode == 0 and os.access(os.path.join(good, 'axio'), os.X_OK))
ok('adds the short names', all(os.readlink(os.path.join(good, n)) == 'axio' for n in ('ax', 'axio-full', 'axio-min')))
