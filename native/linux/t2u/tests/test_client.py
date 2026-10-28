"""Exercise the compiled .so against a local UDP peer; no DVR credentials."""
import ctypes
import hashlib
import os
import socket
import struct
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LIB = ctypes.CDLL(str(ROOT / os.environ.get("SIM_T2U_TEST_BUILD", "build") / "libsim_t2u_linux.so"))


class Result(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint32) for name in
                ("received", "discovered", "mapped", "authenticated", "last_opcode",
                 "key_responses", "v2_responses", "uuid_responses", "v2_error",
                 "peer_notices", "peer_hellos", "peer_authenticated", "relay_registered",
                 "peer_connected", "transport_ready", "stream_opened", "via_relay")]


LIB.sim_t2u_probe.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p,
                              ctypes.c_uint32, ctypes.POINTER(Result)]
LIB.sim_t2u_probe.restype = ctypes.c_int
LIB.tea_encrypt.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
LIB.sim_t2u_peer_probe.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p,
                                   ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint32,
                                   ctypes.POINTER(Result)]
LIB.sim_t2u_peer_probe.restype = ctypes.c_int


class ClientTest(unittest.TestCase):
    def run_peer(self, reply):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            peer.settimeout(2)
            errors = []

            def serve():
                try:
                    for _ in range(3):
                        packet, addr = peer.recvfrom(4096)
                        if struct.unpack_from("<H", packet, 2)[0] == 1012:
                            reply(peer, packet, addr)
                except Exception as exc:
                    errors.append(exc)

            worker = threading.Thread(target=serve)
            worker.start()
            result = Result()
            code = LIB.sim_t2u_probe(b"127.0.0.1", peer.getsockname()[1], b"test-key",
                                      350, ctypes.byref(result))
            worker.join(3)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
            return code, result

    def test_timeout_never_means_authenticated(self):
        start = time.monotonic()
        code, result = self.run_peer(lambda *args: None)
        self.assertEqual(code, -4)
        self.assertEqual(result.authenticated, 0)
        self.assertLess(time.monotonic() - start, 2)

    def test_key_rejected(self):
        def reply(peer, packet, addr):
            peer.sendto(struct.pack("<4I32sI", 0x03F52012, 0, 30, int(time.time()), b"", 0), addr)
        code, result = self.run_peer(reply)
        self.assertEqual(code, -5)
        self.assertEqual(result.authenticated, 0)

    def test_key_response_requires_decryption_and_matching_key(self):
        def reply(peer, packet, addr):
            header = struct.pack("<4I", 0x03F52012, 1, 30, int(time.time()))
            encrypted = ctypes.create_string_buffer(b"test-key".ljust(32, b"\0"), 32)
            LIB.tea_encrypt(encrypted, 32, header)
            peer.sendto(header + encrypted.raw + bytes(4), addr)
        code, result = self.run_peer(reply)
        self.assertEqual(code, 0)
        self.assertEqual(result.authenticated, 1)

    def test_truncated_and_wrong_key_responses_cannot_pass(self):
        def reply(peer, packet, addr):
            peer.sendto(b"\x12\x20\xf5\x03", addr)
            header = struct.pack("<4I", 0x03F52012, 1, 30, int(time.time()))
            encrypted = ctypes.create_string_buffer(b"wrong-key".ljust(32, b"\0"), 32)
            LIB.tea_encrypt(encrypted, 32, header)
            peer.sendto(header + encrypted.raw + bytes(4), addr)
        code, result = self.run_peer(reply)
        self.assertEqual(code, -4)
        self.assertEqual(result.authenticated, 0)

    def test_invalid_input(self):
        result = Result()
        for server, port, key, timeout in [(b"", 1, b"a", 100),
                                            (b"localhost", 0, b"a", 100),
                                            (b"localhost", 1, b"x" * 32, 100),
                                            (b"localhost", 1, b"a", 0)]:
            self.assertEqual(LIB.sim_t2u_probe(server, port, key, timeout,
                                               ctypes.byref(result)), -1)

    def test_direct_peer_login_over_real_local_udp_sockets(self):
        self.peer_login(False)

    def test_relay_registration_precedes_peer_login(self):
        self.peer_login(True)

    def test_keeps_rendezvous_session_while_waiting_for_peer(self):
        self.peer_login(True, defer=5)

    def peer_login(self, relay, defer=0):
        errors = []
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            peer.settimeout(2)

            def serve():
                try:
                    requests = hellos = 0
                    while True:
                        packet, addr = peer.recvfrom(4096)
                        opcode = struct.unpack_from("<H", packet, 2)[0]
                        if opcode == 1012:
                            self.assertEqual(len(packet[48:148].split(b"\0", 1)[0]), 32)
                            header = struct.pack("<4I", 0x03F52012, 1, 30, int(time.time()))
                            encrypted = ctypes.create_string_buffer(b"test-key".ljust(32, b"\0"), 32)
                            LIB.tea_encrypt(encrypted, 32, header)
                            peer.sendto(header + encrypted.raw + bytes(4), addr)
                        elif opcode == 1004:
                            requests += 1
                            if defer:
                                self.assertEqual(requests, 1, "must retain the rendezvous after notice")
                            self.assertEqual(packet[468:500], hashlib.md5(b"test-tunnel").hexdigest().encode())
                            notice = bytearray(588)
                            struct.pack_into("<I", notice, 0, 0x03EC2012)
                            notice[8:108] = b"DEVICE".ljust(100, b"\0")
                            notice[108:208] = packet[8:108]
                            if relay:
                                notice[332:348] = b"127.0.0.1".ljust(16, b"\0")
                                struct.pack_into("<H", notice, 348, peer.getsockname()[1])
                                struct.pack_into("<I", notice, 4, 77)
                            else:
                                notice[208:224] = b"127.0.0.1".ljust(16, b"\0")
                                struct.pack_into("<H", notice, 224, peer.getsockname()[1])
                            peer.sendto(notice, addr)
                        elif opcode == 3001:
                            self.assertTrue(relay)
                            self.assertEqual(len(packet), 212)
                            self.assertEqual(struct.unpack_from("<I", packet, 208)[0], 77)
                            # Relay ACK's value is not an echoed connection ID.
                            peer.sendto(struct.pack("<II", 0x0bba2012, 0), addr)
                        elif opcode == 2000:
                            hellos += 1
                            if hellos <= defer:
                                continue
                            if relay:
                                self.assertEqual(struct.unpack_from("<I", packet, 120)[0], 3)
                            hello = bytearray(184)
                            struct.pack_into("<I", hello, 0, 0x07D12012)
                            hello[8:108] = b"DEVICE".ljust(100, b"\0")
                            peer.sendto(hello, addr)
                        elif opcode == 2007:
                            self.assertEqual(packet[36:72].split(b"\0", 1)[0], b"test-tunnel")
                            peer.sendto(struct.pack("<II16s", 0x07D82012, 1, b""), addr)
                            break
                except Exception as exc:
                    errors.append(exc)

            worker = threading.Thread(target=serve)
            worker.start()
            result = Result()
            code = LIB.sim_t2u_peer_probe(b"127.0.0.1", peer.getsockname()[1], b"test-key",
                                           b"DEVICE", b"test-tunnel", 1800 + defer * 500, ctypes.byref(result))
            worker.join(3)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
            self.assertEqual(code, 0)
            self.assertEqual(result.authenticated, 1)
            self.assertEqual(result.peer_authenticated, 1)


if __name__ == "__main__":
    unittest.main()
