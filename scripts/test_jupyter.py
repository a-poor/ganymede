"""Run Rust integration tests against an isolated, token-protected Jupyter server.

Usage: /path/to/python-with-jupyter scripts/test_jupyter.py
Requires jupyter-server and ipykernel in that Python environment.
"""
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request


def main():
    repo = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="ganymede-jupyter-") as temporary:
        root = Path(temporary)
        for name in ("notebooks", "runtime", "config", "data"):
            (root / name).mkdir()
        token = secrets.token_urlsafe(32)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        env = os.environ.copy()
        env.update({"JUPYTER_RUNTIME_DIR": str(root / "runtime"),
                    "JUPYTER_CONFIG_DIR": str(root / "config"),
                    "JUPYTER_DATA_DIR": str(root / "data"),
                    "IPYTHONDIR": str(root / "ipython")})
        config = root / "config" / "jupyter_server_config.json"
        config.write_text(json.dumps({
            "IdentityProvider": {"token": token},
            "ServerApp": {"ip": "127.0.0.1", "port": port, "port_retries": 0,
                          "open_browser": False, "base_url": "/ganymede-test/",
                          "root_dir": str(root / "notebooks")},
        }))
        config.chmod(0o600)
        url = f"http://127.0.0.1:{port}/ganymede-test/"
        with (root / "server.log").open("w+") as log:
            server = subprocess.Popen([sys.executable, "-m", "jupyter_server", "--config", str(config)],
                                      env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                for _ in range(150):
                    if server.poll() is not None:
                        raise RuntimeError("Jupyter server exited during startup")
                    try:
                        request = urllib.request.Request(url + "api/kernels", headers={"Authorization": "token " + token})
                        with urllib.request.urlopen(request, timeout=1):
                            break
                    except OSError:
                        time.sleep(0.2)
                else:
                    raise RuntimeError("Jupyter server startup timed out")
                env.update({"GANYMEDE_JUPYTER_URL": url, "GANYMEDE_JUPYTER_TOKEN": token})
                result = subprocess.run(["cargo", "test", "-p", "ganymede-core", "--test", "jupyter_live",
                                         "--", "--ignored", "--nocapture"], cwd=repo, env=env, timeout=180)
                if result.returncode == 0:
                    result = subprocess.run(["cargo", "run", "-p", "ganymede-core", "--example", "execute",
                                             "--", "print('example smoke test'); 6 * 7"],
                                            cwd=repo, env=env, timeout=90)
                if result.returncode:
                    log.seek(0)
                    print(log.read().replace(token, "[REDACTED]"), file=sys.stderr)
                if result.returncode == 0:
                    result = subprocess.run(["cargo", "run", "-p", "ganymede-core", "--example", "notebook"],
                                            cwd=repo, env=env, timeout=90)
                return result.returncode
            finally:
                # Include kernels in cleanup, even when a Rust assertion fails.
                try:
                    os.killpg(server.pid, signal.SIGTERM)
                    server.wait(timeout=10)
                except (ProcessLookupError, subprocess.TimeoutExpired):
                    try:
                        os.killpg(server.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    server.wait()


if __name__ == "__main__":
    sys.exit(main())
