"""Install one built wheel outside the repository and exercise its import-only safe surface."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


def run(*args: str, cwd: Path) -> None:
    subprocess.run(args, cwd=cwd, check=True)


def main() -> int:
    wheel = Path(sys.argv[1]).resolve()
    if not wheel.is_file() or wheel.suffix != ".whl":
        raise RuntimeError("packed consumer needs one built wheel")
    with tempfile.TemporaryDirectory(prefix="runtrol-python-consumer-") as temporary:
        root = Path(temporary)
        environment = root / "venv"
        run(sys.executable, "-m", "venv", str(environment), cwd=root)
        python = environment / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
        run(
            str(python),
            "-m",
            "pip",
            "install",
            "--only-binary=:all:",
            "--no-deps",
            str(wheel),
            cwd=root,
        )
        probe = (
            "import asyncio, json, os, pathlib\n"
            "import runtrol_runtime as rr\n"
            "identity = rr.Identity.generate()\n"
            "assert len(identity.secret_bytes()) == 32\n"
            "absent = pathlib.Path.cwd() / 'absent-runtime'\n"
            "os.environ['RUNTROL_HOME'] = str(absent)\n"
            "async def probe_absence():\n"
            "    try:\n"
            "        await rr.AsyncRuntimeClient.connect(name='packed-consumer', version='1', identity=identity)\n"
            "    except rr.RuntimeNotInstalledError as error:\n"
            "        assert error.code == 'runtimeNotInstalled'\n"
            "        return\n"
            "    raise AssertionError('connect unexpectedly found Runtime')\n"
            "asyncio.run(probe_absence())\n"
            "assert not absent.exists()\n"
            "print(json.dumps({'key': identity.public_key_base64(), 'typed': hasattr(rr, 'AsyncRuntimeClient')}))\n"
        )
        completed = subprocess.run(
            [str(python), "-I", "-c", probe],
            cwd=root,
            check=True,
            text=True,
            capture_output=True,
        )
        result = json.loads(completed.stdout)
        if not result.get("typed") or not isinstance(result.get("key"), str):
            raise RuntimeError("the packed client did not expose its typed identity surface")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
