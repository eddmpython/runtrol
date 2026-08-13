"""Gate: a headless phone answers a real provider approval and the same turn resumes."""

from __future__ import annotations

import sys

import phoneDrivesPcSmoke as phone


if __name__ == "__main__":
    raise SystemExit(phone.run("approval", sys.argv[1:], "approvalRoundtripSmoke"))
