"""Read coordinator JSON from stdin. Never print keys, serials or packet data.

Exit 0 only validates service authentication; it is NOT the device tunnel gate.
"""
import ctypes
import json
import sys
import struct
import time
from test_client import LIB, Result

config = json.load(sys.stdin)
result = Result()
tunnel_verified = False
sdk_login = None
if config.get("serial"):
    LIB.sim_t2u_open.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p,
                                 ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                                 ctypes.c_uint16, ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p),
                                 ctypes.POINTER(Result)]
    LIB.sim_t2u_write.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
    LIB.sim_t2u_read.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
    LIB.sim_t2u_close.argtypes = [ctypes.c_void_p]
    channel = ctypes.c_void_p()
    code = LIB.sim_t2u_open(config["server"].encode(), config["port"],
                            config["key"].encode(), config["serial"].upper().encode(),
                            config["tunnel_password"].encode(), config["remote_ip"].encode(),
                            37777, 15000, ctypes.byref(channel), ctypes.byref(result))
    if code == 0:
        try:
            challenge = bytearray(32)
            challenge[0:2] = b'\xa0\x01'
            challenge[24:32] = bytes([5,2,1,1,0,0,0xa1,0xaa])
            sent = LIB.sim_t2u_write(channel, bytes(challenge), len(challenge), 8000)
            response = ctypes.create_string_buffer(4096)
            reply = bytearray()
            expected = 32
            count = sent
            deadline = time.monotonic() + 8
            while sent == 32 and len(reply) < expected:
                left_ms = int((deadline - time.monotonic()) * 1000)
                if left_ms <= 0:
                    count = -4
                    break
                count = LIB.sim_t2u_read(channel, response, min(len(response), expected - len(reply)), left_ms)
                if count <= 0:
                    break
                reply.extend(response.raw[:count])
                if len(reply) == 32:
                    expected = 32 + struct.unpack_from("<I", reply, 4)[0]
                    if expected > 65536:
                        count = -6
                        break
            tunnel_verified = (count > 0 and len(reply) == expected and reply[0] == 0xb0
                               and b"Realm:" in reply[32:] and b"Random:" in reply[32:])
            if not tunnel_verified:
                code = count if count < 0 else -6
            # The challenge belongs to this application connection. Open a fresh
            # channel for the official SDK, which must perform its own login.
        finally:
            LIB.sim_t2u_close(channel)
        if code == 0 and config.get("username") and "password" in config:
            code = LIB.sim_t2u_open(config["server"].encode(), config["port"],
                                    config["key"].encode(), config["serial"].upper().encode(),
                                    config["tunnel_password"].encode(), config["remote_ip"].encode(),
                                    37777, 15000, ctypes.byref(channel), ctypes.byref(result))
            if code == 0:
                try:
                    from sdk_login import verify_sdk
                    sdk_login = verify_sdk(LIB, channel, config)
                    if sdk_login["code"] or sdk_login["transport_errors"]:
                        code = -6
                finally:
                    LIB.sim_t2u_close(channel)
else:
    code = LIB.sim_t2u_probe(config["server"].encode(), config["port"],
                              config["key"].encode(), 15000, ctypes.byref(result))
print(json.dumps({"code": code, **{name: getattr(result, name) for name, _ in result._fields_},
                  "device_tunnel_verified": tunnel_verified, "sdk_login": sdk_login}))
sys.exit(0 if code == 0 else 1)
