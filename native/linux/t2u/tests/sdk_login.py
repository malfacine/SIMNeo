"""Temporary loopback adapter for validating the official SDK over native T2U.

The adapter belongs to the test process; it accepts one connection, on localhost.
Credentials live only in memory. No SDK call changes device configuration.
"""
import ctypes
import select
import socket
import threading
from pathlib import Path


def verify_sdk(lib, channel, profile):
    class Result(ctypes.Structure):
        _fields_ = [(name, ctypes.c_uint32) for name in
                    ("sdk_version", "initialized", "logged_in", "last_error")]
        _fields_ += [("login_error", ctypes.c_int32), ("channels", ctypes.c_int32)]

    path = Path(__file__).resolve().parents[2] / "netsdk/build/libsim_intelbras_linux.so"
    sdk = ctypes.CDLL(str(path))
    sdk.sim_intelbras_probe.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p,
                                         ctypes.c_char_p, ctypes.c_int, ctypes.POINTER(Result)]
    stop = threading.Event()
    errors = []
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(8)

    def forward():
        try:
            connection, _ = listener.accept()
            with connection:
                while not stop.is_set():
                    if select.select([connection], [], [], 0)[0]:
                        data = connection.recv(4096)
                        if not data:
                            break
                        written = lib.sim_t2u_write(channel, data, len(data), 8000)
                        if written != len(data):
                            errors.append(written)
                            break
                    response = ctypes.create_string_buffer(4096)
                    count = lib.sim_t2u_read(channel, response, len(response), 100)
                    if count > 0:
                        connection.sendall(response.raw[:count])
                    elif count != -4:
                        if count < 0:
                            errors.append(count)
                        break
        except (BrokenPipeError, ConnectionResetError):
            # SDK logout/cleanup closes its TCP side; this is not UDP failure.
            pass
        except OSError:
            errors.append("socket_error")
        finally:
            listener.close()

    port = listener.getsockname()[1]
    worker = threading.Thread(target=forward)
    worker.start()
    result = Result()
    try:
        code = sdk.sim_intelbras_probe(b"127.0.0.1", port, profile["username"].encode(),
                                       profile["password"].encode(), 0, ctypes.byref(result))
    finally:
        stop.set()
        worker.join(10)
    return {"code": code, **{name: getattr(result, name) for name, _ in result._fields_},
            "transport_errors": errors, "worker_stopped": not worker.is_alive()}
