"""Transport tests with UDP loss, duplication and malformed frames."""
import ctypes
import socket
import struct
import threading
import time
import unittest
from test_client import LIB


class Address(ctypes.Structure):
    _fields_ = [("family", ctypes.c_ushort), ("port", ctypes.c_ushort),
                ("ip", ctypes.c_ubyte * 4), ("zero", ctypes.c_ubyte * 8)]


LIB.vdt_open.argtypes = [ctypes.c_int, ctypes.POINTER(Address), ctypes.c_char_p,
                         ctypes.c_uint32, ctypes.c_char_p, ctypes.c_uint16, ctypes.c_uint32,
                         ctypes.c_void_p, ctypes.c_void_p]
LIB.vdt_open.restype = ctypes.c_void_p
LIB.sim_t2u_write.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
LIB.sim_t2u_read.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
LIB.sim_t2u_close.argtypes = [ctypes.c_void_p]


def frame(kind, payload=b"", sequence=0):
    header = bytearray(16)
    struct.pack_into("!HHI", header, 0, 0x1201, 0x1000 | kind, sequence)
    if kind == 2:
        struct.pack_into("!H", header, 8, len(payload))
        header[10] = 1
        header[12] = 1
    elif kind == 3:
        header[4] = 1
        header[5] = 101
        struct.pack_into("!I", header, 8, sequence)
    return bytes(header[:12] if kind == 7 else header) + payload


class TransportTest(unittest.TestCase):
    def coordinator(self, channel, rejected=False):
        server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        server.bind(("127.0.0.1", 0))
        server.settimeout(0.2)
        stop = threading.Event()
        packets, errors = [], []
        LIB.tea_decrypt.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]

        def serve():
            try:
                while not stop.is_set():
                    try:
                        packet, sender = server.recvfrom(2048)
                    except socket.timeout:
                        continue
                    self.assertEqual(len(packet), 160)
                    self.assertEqual(struct.unpack_from("<I", packet)[0], 0x03f42012)
                    encrypted = ctypes.create_string_buffer(packet[16:48], 32)
                    self.assertNotEqual(encrypted.raw, b"test-key".ljust(32, b"\0"))
                    LIB.tea_decrypt(encrypted, 32, packet[:16])
                    self.assertEqual(encrypted.raw.split(b"\0", 1)[0], b"test-key")
                    self.assertEqual(packet[48:148].split(b"\0", 1)[0], b"test-client")
                    packets.append(packet)
                    header = struct.pack("<4I", 0x03f52012, 0 if rejected else 1, 1, int(time.time()))
                    payload = ctypes.create_string_buffer(b"test-key".ljust(32, b"\0"), 32)
                    LIB.tea_encrypt(payload, 32, header)
                    server.sendto(header + payload.raw + bytes(4), sender)
            except BaseException as exc:
                errors.append(exc)
            finally:
                server.close()

        address = Address(socket.AF_INET, socket.htons(server.getsockname()[1]),
                          (ctypes.c_ubyte * 4)(127, 0, 0, 1), (ctypes.c_ubyte * 8)())
        configure = LIB.vdt_configure_service
        configure.argtypes = [ctypes.c_void_p, ctypes.POINTER(Address), ctypes.c_char_p, ctypes.c_uint32]
        self.assertEqual(configure(channel, ctypes.byref(address), b"test-key", 100), 0)
        worker = threading.Thread(target=serve)
        worker.start()

        def finish():
            stop.set()
            worker.join(1)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
        self.addCleanup(finish)
        return packets

    def test_coordinator_authentication_is_renewed_after_open(self):
        channel = self.session(lambda *args: None)
        packets = self.coordinator(channel)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 700), -4)
        self.assertGreaterEqual(len(packets), 2)

    def test_coordinator_revocation_is_reported_during_stream(self):
        channel = self.session(lambda *args: None)
        self.coordinator(channel, rejected=True)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 700), -5)

    def session(self, responder, on_ack=None):
        peer = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        peer.bind(("127.0.0.1", 0))
        peer.settimeout(4)
        client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        client.bind(("127.0.0.1", 0))
        client.setblocking(False)
        fd = client.detach()
        errors = []
        closed = threading.Event()
        transport_closed = threading.Event()
        self.peer_alive = threading.Event()
        self.peer_heartbeats = []
        self.acks = []
        self.handshakes = []

        def serve():
            try:
                while True:
                    packet, sender = peer.recvfrom(2048)
                    if packet[:4] == struct.pack("<I", 0x07d22012):
                        words = struct.unpack("<6I", packet)
                        self.assertEqual(words[0], 0x07d22012)
                        self.assertEqual(words[2:], (121, 1, 0, 0))
                        self.peer_heartbeats.append(packet)
                        self.peer_alive.set()
                        continue
                    kind = packet[3]
                    if kind == 0:
                        self.handshakes.append(packet)
                        peer.sendto(packet[:3] + b"\x01" + packet[4:], sender)
                    elif kind == 2:
                        sequence = struct.unpack_from("!I", packet, 4)[0]
                        body = packet[16:]
                        command = struct.unpack_from("<I", body)[0]
                        if command == 0xff01ffff:
                            self.assertEqual(struct.unpack_from("!H", body, 8)[0], 1)
                            self.assertEqual(body[10:20], b"127.0.0.1\0")
                            self.assertEqual(struct.unpack_from("!H", body, 30)[0], 37777)
                            peer.sendto(frame(3, sequence=sequence), sender)
                        elif command == 0xff02ffff:
                            closed.set()
                        else:
                            responder(peer, sender, packet, sequence)
                    elif kind == 3:
                        self.acks.append(packet)
                        if on_ack:
                            on_ack(peer, sender, packet)
                    elif kind == 7:
                        self.assertEqual(len(packet), 12)
                        transport_closed.set()
                        return
            except BaseException as exc:
                errors.append(exc)
            finally:
                peer.close()

        worker = threading.Thread(target=serve)
        worker.start()
        address = Address(socket.AF_INET, socket.htons(peer.getsockname()[1]),
                          (ctypes.c_ubyte * 4)(127, 0, 0, 1), (ctypes.c_ubyte * 8)())
        channel = LIB.vdt_open(fd, ctypes.byref(address), b"test-client", 1,
                               b"127.0.0.1", 37777, 1500, None, None)
        if not channel:
            socket.close(fd)
        self.assertTrue(channel)
        self.addCleanup(self.finish, channel, worker, closed, transport_closed, errors)
        return channel

    def test_ack_request_recovers_missing_tail(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(frame(4, sequence=1), sender)

        def ack(peer, sender, packet):
            count = struct.unpack_from("!H", packet, 6)[0]
            if count:
                self.assertEqual(packet[16:], struct.pack("!I", 1))
                peer.sendto(frame(2, struct.pack("<II", 1, 5) + b"reply", 1), sender)

        channel = self.session(respond, ack)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 5)
        self.assertEqual(output.raw[:5], b"reply")

    def test_out_of_order_discarded_data_is_requested_again(self):
        sent = threading.Event()
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(frame(2, b"ply", 2), sender)

        def ack(peer, sender, packet):
            count = struct.unpack_from("!H", packet, 6)[0]
            if count and not sent.is_set():
                self.assertEqual(packet[16:], struct.pack("!II", 1, 2))
                sent.set()
                peer.sendto(frame(2, struct.pack("<II", 1, 5) + b"re", 1), sender)
                peer.sendto(frame(2, b"ply", 2), sender)

        channel = self.session(respond, ack)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        result = b""
        while len(result) < 5:
            size = LIB.sim_t2u_read(channel, output, 32, 500)
            self.assertGreater(size, 0)
            result += output.raw[:size]
        self.assertEqual(result, b"reply")

    def test_retransmission_counter_is_valid_in_data_header(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            reply = bytearray(frame(2, struct.pack("<II", 1, 5) + b"reply", 1))
            reply[11] = 3
            peer.sendto(reply, sender)
        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 5)
        self.assertEqual(output.raw[:5], b"reply")

    def test_ack_request_with_large_sequence_has_bounded_response(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(frame(4, sequence=0xffffffff), sender)
        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 100), -4)
        self.assertTrue(self.acks)
        ack = self.acks[-1]
        self.assertEqual(len(ack), 16 + 256 * 4)
        self.assertEqual(struct.unpack_from("!H", ack, 6)[0], 256)
        self.assertEqual(struct.unpack_from("!I", ack, 16)[0], 1)
        self.assertEqual(struct.unpack_from("!I", ack, len(ack) - 4)[0], 256)

    def finish(self, channel, worker, closed, transport_closed, errors):
        LIB.sim_t2u_close(channel)
        worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertTrue(closed.is_set())
        self.assertTrue(transport_closed.is_set(), "VDT FIN was not sent after closing the application channel")
        self.assertEqual(errors, [])

    def test_close_finishes_transport_not_only_application(self):
        self.session(lambda *args: None)

    def test_emits_peer_keepalive_alongside_transport_heartbeat(self):
        self.session(lambda *args: None)
        self.assertTrue(self.peer_alive.wait(1), "Peer keepalive was not sent")

    def test_peer_keepalive_does_not_close_stream(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(struct.pack("<6I", 0x07d22012, 5, 121, 1, 0, 0), sender)
            peer.sendto(frame(2, struct.pack("<II", 1, 5) + b"reply", 1), sender)
        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 5)
        self.assertEqual(output.raw[:5], b"reply")

    def test_peer_keepalive_is_renewed_while_idle(self):
        channel = self.session(lambda *args: None)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 3400), -4)
        self.assertGreaterEqual(len(self.peer_heartbeats), 2)
        self.assertEqual([struct.unpack_from("<I", packet, 4)[0] for packet in self.peer_heartbeats],
                         list(range(1, len(self.peer_heartbeats) + 1)))
        self.assertEqual(len(self.handshakes), 1, "Connected peers must not renegotiate VDT every heartbeat")
        maximum, available = struct.unpack_from("!HH", self.handshakes[0], 16)
        self.assertGreaterEqual(maximum, 20)
        self.assertLessEqual(maximum, 256)
        self.assertEqual(maximum, available)

    def test_peer_finish_closes_read_without_waiting_for_timeout(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(frame(7), sender)
        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        started = time.monotonic()
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 0)
        self.assertLess(time.monotonic() - started, 0.2)

    def test_retransmits_lost_packet_and_deduplicates_reply(self):
        attempts = []

        def respond(peer, sender, packet, sequence):
            attempts.append(packet)
            if len(attempts) == 1:
                return
            self.assertEqual(packet, attempts[0])
            peer.sendto(frame(3, sequence=sequence), sender)
            response = frame(2, struct.pack("<II", 1, 5) + b"reply", 1)
            peer.sendto(response, sender)
            peer.sendto(response, sender)

        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 5)
        self.assertEqual(output.raw[:5], b"reply")
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 150), -4)
        self.assertEqual(len(attempts), 2)

    def test_ack_advertises_window_within_linux_buffer_capacity(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            peer.sendto(frame(2, struct.pack("<II", 1, 5) + b"reply", 1), sender)

        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), 5)
        deadline = time.monotonic() + 1
        while not self.acks and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(self.acks)
        # SDK: uint16 receive window at +4, uint16 SACK count at +6.
        # Linux retains 512 KiB and accepts datagrams up to 2048 bytes.
        window, missing = struct.unpack_from("!HH", self.acks[0], 4)
        self.assertGreater(window, 0)
        self.assertLessEqual(window, 256)
        self.assertEqual(missing, 0)
        self.assertEqual(struct.unpack_from("!I", self.acks[0], 8)[0], 1)

    def test_splits_large_write_without_changing_stream_bytes(self):
        captured = bytearray()

        def respond(peer, sender, packet, sequence):
            captured.extend(packet[24:])
            peer.sendto(frame(3, sequence=sequence), sender)

        channel = self.session(respond)
        data = bytes(range(256)) * 13
        self.assertEqual(LIB.sim_t2u_write(channel, data, len(data), 1500), len(data))
        self.assertEqual(captured, data)

    def test_receive_window_shrinks_without_losing_buffered_stream_bytes(self):
        data = bytes(range(250)) * 240
        response = struct.pack("<II", 1, len(data)) + data
        packets = (len(response) + 1399) // 1400

        def respond(peer, sender, packet, sequence):
            # Fill the receive buffer while write() is waiting for its ACK.
            for index in range(packets):
                peer.sendto(frame(2, response[index*1400:(index+1)*1400], index+1), sender)
            peer.sendto(frame(3, sequence=sequence), sender)

        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(65536)
        self.assertEqual(LIB.sim_t2u_read(channel, output, len(output), 500), len(data))
        self.assertEqual(output.raw[:len(data)], data)
        deadline = time.monotonic() + 1
        while len(self.acks) < packets and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(len(self.acks), packets)
        windows = [struct.unpack_from("!H", packet, 4)[0] for packet in self.acks]
        self.assertTrue(all(1 <= window <= 256 for window in windows))
        self.assertGreater(windows[0], windows[-1])

    def test_receives_burst_larger_than_write_limit_while_waiting_for_ack(self):
        data = bytes(range(250)) * 640
        response = struct.pack("<II", 1, len(data)) + data
        packets = [response[i:i+1400] for i in range(0, len(response), 1400)]
        request_sequence = [0]

        def respond(peer, sender, packet, sequence):
            request_sequence[0] = sequence
            peer.sendto(frame(2, packets[0], 1), sender)

        def ack(peer, sender, packet):
            received = struct.unpack_from("!I", packet, 8)[0]
            if received == len(packets):
                peer.sendto(frame(3, sequence=request_sequence[0]), sender)
            elif received:
                peer.sendto(frame(2, packets[received], received + 1), sender)

        channel = self.session(respond, ack)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(65536)
        collected = b""
        while len(collected) < len(data):
            count = LIB.sim_t2u_read(channel, output, len(output), 500)
            self.assertGreater(count, 0)
            collected += output.raw[:count]
        self.assertEqual(collected, data)
        self.assertEqual(LIB.sim_t2u_write(channel, output, 65537, 500), -1)

    def test_rejects_truncated_transport_payload(self):
        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            damaged = bytearray(frame(2, struct.pack("<II", 1, 5) + b"reply", 1))
            damaged.pop()
            peer.sendto(damaged, sender)

        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        output = ctypes.create_string_buffer(32)
        self.assertEqual(LIB.sim_t2u_read(channel, output, 32, 500), -6)

    def test_preserves_record_split_across_transport_packets(self):
        reply = b"fragmented-stream" * 100

        def respond(peer, sender, packet, sequence):
            peer.sendto(frame(3, sequence=sequence), sender)
            body = struct.pack("<II", 1, len(reply)) + reply
            parts = [body[:4], body[4:1000], body[1000:]]
            for i, part in enumerate(parts, 1):
                datagram = bytearray(frame(2, part, i))
                if i > 1:
                    datagram[10] = 0
                peer.sendto(datagram, sender)

        channel = self.session(respond)
        self.assertEqual(LIB.sim_t2u_write(channel, b"request", 7, 1500), 7)
        collected = bytearray()
        output = ctypes.create_string_buffer(400)
        while len(collected) < len(reply):
            count = LIB.sim_t2u_read(channel, output, len(output), 500)
            self.assertGreater(count, 0)
            collected.extend(output.raw[:count])
        self.assertEqual(collected, reply)


if __name__ == "__main__":
    unittest.main()
