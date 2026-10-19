#!/usr/bin/env python3
"""Generate a username/UUID API key and its allowlist digest."""

import base64
import hashlib
import json
import sys
import uuid


def main():
    try:
        username = input("Username: ").strip()
    except (EOFError, KeyboardInterrupt):
        print("\nNo username provided.", file=sys.stderr)
        return 1
    if not username:
        print("Username must not be empty.", file=sys.stderr)
        return 1

    payload = json.dumps(
        {"username": username, "uuid": str(uuid.uuid4())},
        ensure_ascii=False,
        separators=(",", ":"),
    ).encode("utf-8")
    api_key = base64.urlsafe_b64encode(payload).rstrip(b"=").decode("ascii")
    digest = hashlib.sha256(api_key.encode("ascii")).hexdigest()

    print(f"API key: {api_key}")
    print(f"SHA-256: {digest}")
    record = json.dumps({"username": username}, ensure_ascii=False)
    print(f"Allowlist record: {record}")
    print("Add this hash and username to your deployment's API key allowlist storage.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
