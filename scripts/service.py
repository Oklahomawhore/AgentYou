#!/usr/bin/env python3
"""Start/stop only this workspace's local server; no API secrets in argv or logs."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / '.local'
BINARY = ROOT / 'target/debug/yourself-server'
PIDFILE = DATA / 'server.pid'
PORT = int(os.environ.get('YOURSELF_PORT', '4317'))
URL = f'http://127.0.0.1:{PORT}'

def owned_pid():
    if not PIDFILE.exists():
        return None
    try:
        pid = int(PIDFILE.read_text().strip())
        command = subprocess.check_output(['/bin/ps', '-p', str(pid), '-o', 'command='], text=True).strip()
        return pid if command.startswith(str(BINARY) + ' ') else None
    except (ValueError, subprocess.CalledProcessError):
        return None

def healthy():
    try:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(URL + '/health', timeout=1) as response:
            return json.load(response).get('service') == 'yourself'
    except Exception:
        return False

command = sys.argv[1] if len(sys.argv) > 1 else 'status'
if command == 'start':
    DATA.mkdir(exist_ok=True, mode=0o700)
    os.chmod(DATA, 0o700)
    if owned_pid():
        print(f'Already running: {URL}')
        sys.exit(0)
    if not BINARY.exists():
        sys.exit('Build first: cargo build --locked -p yourself-server')
    with (DATA / 'server.log').open('ab') as log:
        process = subprocess.Popen([str(BINARY), '--port', str(PORT), '--data-dir', str(DATA)],
                                   cwd=ROOT, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                   start_new_session=True, close_fds=True)
    PIDFILE.write_text(str(process.pid))
    for _ in range(50):
        if process.poll() is not None:
            sys.exit(f'Service exited. Check {DATA / "server.log"}')
        if healthy():
            print(f'Ready: {URL}')
            break
        time.sleep(0.1)
    else:
        sys.exit(f'Service did not become healthy. Check {DATA / "server.log"}')
elif command == 'stop':
    pid = owned_pid()
    if pid:
        os.kill(pid, signal.SIGINT)
        for _ in range(100):
            if not owned_pid():
                break
            time.sleep(0.1)
        else:
            sys.exit('Server is still shutting down; pending requests may take up to 90 seconds.')
        PIDFILE.unlink(missing_ok=True)
        print('Stopped.')
    else:
        print('Not running.')
elif command == 'status':
    print(f'Running: {URL}' if owned_pid() and healthy() else 'Not running.')
else:
    sys.exit('Usage: python3 scripts/service.py start|stop|status')
