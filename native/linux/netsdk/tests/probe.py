"""Read device JSON on stdin; print numeric results only, never credentials."""
import ctypes
import json
from pathlib import Path
import sys


class Result(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint32) for name in
                ("sdk_version", "initialized", "logged_in", "last_error")]
    _fields_ += [("login_error", ctypes.c_int32), ("channels", ctypes.c_int32)]


lib = ctypes.CDLL(str(Path(__file__).resolve().parents[1] / "build/libsim_intelbras_linux.so"))
lib.sim_intelbras_probe.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p,
                                     ctypes.c_char_p, ctypes.c_int, ctypes.POINTER(Result)]
lib.sim_intelbras_probe.restype = ctypes.c_int
result = Result()
if sys.argv[1:] == ["--init"]:
    code = lib.sim_intelbras_probe(None, 0, None, None, 0, ctypes.byref(result))
else:
    profile = json.load(sys.stdin)
    host = profile.get("host") or profile.get("serial", "").upper()
    mode = int(sys.argv[1]) if len(sys.argv) > 1 else 19
    code = lib.sim_intelbras_probe(host.encode(), profile.get("port", 37777),
                                   profile["username"].encode(), profile["password"].encode(),
                                   mode, ctypes.byref(result))
print(json.dumps({"code": code, **{name: getattr(result, name) for name, _ in result._fields_}}))
sys.exit(0 if code == 0 else 1)
