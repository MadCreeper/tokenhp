"""Native OpenSSH + production askpass executable, against a localhost-only SSH
server that accepts password auth/forwarding and explicitly refuses shells.
No user credentials, SSH configuration, or database is read or modified.
"""
import argparse
import os
import socket
import subprocess
import threading
import paramiko

parser = argparse.ArgumentParser()
parser.add_argument('--target')
args = parser.parse_args()
password = '  test !%&^ $\\"\' 密码🔑 trailing '
host_key = paramiko.RSAKey.generate(2048)
stop = threading.Event()
connections = []

class Server(paramiko.ServerInterface):
    def check_auth_password(self, username, candidate):
        return paramiko.AUTH_SUCCESSFUL if username == 'hpbar-smoke' and candidate == password else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return 'password'

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        return paramiko.OPEN_SUCCEEDED if destination == ('127.0.0.1', 5432) else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_request(self, kind, chanid):
        # Like a tunnel-only nologin account: no session/shell permitted.
        return paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED


def serve(sock):
    transport = paramiko.Transport(sock)
    connections.append(transport)
    try:
        transport.add_server_key(host_key)
        transport.start_server(server=Server())
        while transport.is_active() and not stop.is_set():
            channel = transport.accept(timeout=0.2)
            if channel is not None:
                with channel:
                    channel.settimeout(5)
                    data = channel.recv(1024)
                    if data:
                        channel.sendall(data)
    except (EOFError, OSError, paramiko.SSHException):
        pass
    finally:
        transport.close()

listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen(5)
listener.settimeout(0.2)

def accept():
    while not stop.is_set():
        try:
            sock, _ = listener.accept()
        except socket.timeout:
            continue
        except OSError:
            return
        threading.Thread(target=serve, args=(sock,), daemon=True).start()

threading.Thread(target=accept, daemon=True).start()
env = os.environ.copy()
env['HPBAR_TEST_SSH_PORT'] = str(listener.getsockname()[1])
env['HPBAR_TEST_SSH_PASSWORD'] = password
cmd = ['cargo', 'test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--release', '--test', 'platform_processes']
if args.target:
    cmd += ['--target', args.target]
cmd += ['--', '--include-ignored', '--nocapture']
try:
    raise SystemExit(subprocess.run(cmd, env=env).returncode)
finally:
    stop.set()
    listener.close()
    for transport in connections:
        transport.close()
