#!/usr/bin/env python3
"""Careful park/restore for the bench unit after a failed push.

Read-only check of what's on the device, then — only if asked --
restore the exact original bytes from a backup dir, using the
legacy installer's verified write encoding (base64 chunks, 384 B binary) instead
of the escaped-literal path that MemoryError'd.

    python scripts/park_device.py COM12 backups/COM12-20260828-213632
    python scripts/park_device.py COM12 backups/... --restore
"""

import base64
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import importlib.util

spec = importlib.util.spec_from_file_location(
    "pf", os.path.join(os.path.dirname(os.path.abspath(__file__)), "push_firmware.py")
)
pf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pf)


def write_file_b64(r, name, data):
    """Encode 384-byte binary chunks as 512-character base64 blocks.
    per chunk. It provisioned these very units; it stays the reference
    until the escaped path earns its keep."""
    r.exec("import gc; gc.collect()")
    r.exec("import ubinascii")
    r.exec("f = open('%s','wb')" % name)
    b64 = base64.b64encode(data).decode()
    for i in range(0, len(b64), 512):
        r.exec("f.write(ubinascii.a2b_base64('%s'))" % b64[i : i + 512])
    r.exec("f.close()")


def verify(r, name, data):
    got = r.read_file(name)
    ok = got == data
    print("  verify %-22s %s (%d bytes)" % (name, "OK" if ok else "MISMATCH", len(got)))
    return ok


def main():
    port = sys.argv[1]
    backup_dir = sys.argv[2] if len(sys.argv) > 2 else None
    restore = "--restore" in sys.argv

    r = pf.Repl(port)
    files = r.list_files()
    print("device files:", files)

    stats = {}
    for name in files:
        try:
            out = r.exec(
                "import os; print(os.stat('%s')[6])" % name
            )
            m = re.search(rb"(\d+)", out)
            stats[name] = int(m.group(1)) if m else -1
        except SystemExit as e:
            print("  stat failed for %s: %s" % (name, str(e).strip().splitlines()[-1]))
            stats[name] = -1
    for name, size in stats.items():
        print("  %-22s %s" % (name, size))

    if not restore or not backup_dir:
        print("read-only check done (pass --restore to write the backup back)")
        return

    originals = sorted(os.listdir(backup_dir))
    print("restoring %d original files from %s" % (len(originals), backup_dir))
    good = True
    for name in originals:
        with open(os.path.join(backup_dir, name), "rb") as f:
            data = f.read()
        write_file_b64(r, name, data)
        good = verify(r, name, data) and good
    r.soft_reboot()
    print("restore %s — device rebooted into its original firmware"
          % ("verified" if good else "HAD MISMATCHES"))


if __name__ == "__main__":
    main()
