"""Compare Linux TEA with the pinned Windows binary, with synthetic input only.

Run on Windows: python tests/compare_dll.py [WSL distribution]
Run on Linux internally: python tests/compare_dll.py --linux < vectors.json
"""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

if sys.argv[1:2] == ["--linux"]:
    lib = ctypes.CDLL(str(ROOT / "build/libsim_t2u_linux.so"))
    lib.tea_encrypt.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
    vectors = json.load(sys.stdin)
    for vector in vectors:
        key = bytes.fromhex(vector["key"])
        raw = bytes.fromhex(vector["raw"])
        padded = raw.ljust((len(raw) + 7) // 8 * 8, b"\0")
        buffer = ctypes.create_string_buffer(padded, len(padded))
        lib.tea_encrypt(buffer, len(padded), key)
        assert buffer.raw.hex() == vector["expected"], f"TEA mismatch: length {len(raw)}"
    print(f"Linux TEA matches pinned Windows DLL: {len(vectors)} synthetic vectors")
elif os.name == "nt":
    path = ROOT.parents[2] / "assets/libt2u.dll"
    assert hashlib.sha256(path.read_bytes()).hexdigest() == (
        "528b8289c6f59e447c91508790d21abe888ad16ee1b1f6e30a52c4799bbf4d02")
    dll = ctypes.CDLL(str(path))
    # Verified RVA of tea_encrypt_padded in the pinned binary. No t2u_init/network.
    encrypt = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_int,
                              ctypes.c_void_p)(dll._handle + 0xFD00)
    vectors = []
    for n in (1, 7, 8, 9, 16, 20, 31, 32, 33, 100, 1300):
        key, raw = os.urandom(16), os.urandom(n)
        padded = raw.ljust((n + 7) // 8 * 8, b"\0")
        buffer = ctypes.create_string_buffer(padded, len(padded))
        size = encrypt(buffer, n, key)
        assert size == len(padded)
        vectors.append({"key": key.hex(), "raw": raw.hex(), "expected": buffer.raw.hex()})
    distro = sys.argv[1] if len(sys.argv) > 1 else "Debian"
    linux_path = subprocess.check_output(["wsl", "-d", distro, "--exec", "wslpath", "-a", Path(__file__).as_posix()], text=True).strip()
    process = subprocess.run(["wsl", "-d", distro, "--", "python3", linux_path, "--linux"],
                             input=json.dumps(vectors), text=True)
    sys.exit(process.returncode)
else:
    raise SystemExit("Run on Windows for differential verification against libt2u.dll")
