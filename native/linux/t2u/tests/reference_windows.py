"""Read coordinator via stdin and inspect only the pinned DLL's service status."""
import ctypes
import hashlib
import json
from pathlib import Path
import sys
import time
import struct
import socket

config = json.load(sys.stdin)
started = time.monotonic()
path = Path(__file__).resolve().parents[4] / "assets/libt2u.dll"
assert hashlib.sha256(path.read_bytes()).hexdigest() == (
    "528b8289c6f59e447c91508790d21abe888ad16ee1b1f6e30a52c4799bbf4d02")
dll = ctypes.CDLL(str(path))
observed = []
incoming = []
stream_metadata = []
transport_metadata = []
ack_metadata = []
# Observe this test process's sendto imports only. Never alter the DLL on disk,
# never save packet data, and forward each call to the original Winsock function.
send_type = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_size_t, ctypes.c_void_p,
                               ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_int)
slot = ctypes.c_void_p.from_address(dll._handle + 0x40360)
original_send = send_type(slot.value)
decrypt = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_int,
                          ctypes.c_void_p)(dll._handle + 0xFE10)


@send_type
def observe(sock, data, size, flags, address, address_size):
    if size >= 4:
        packet = ctypes.string_at(data, size)
        if packet[:4] == b'\x12\x01\x10\x03' and len(ack_metadata) < 30:
            ack_metadata.append({"size": size, "header": packet[:64].hex()})
        if packet[:2] == b"\x12\x01" and len(transport_metadata) < 60:
            item = {"size": size, "header": packet[:16].hex()}
            if packet[:2] == b'\x12\x01' and packet[3] == 2:
                item["t2u_header"] = packet[16:24].hex()
                if size >= 48 and struct.unpack_from("<I", packet, 16)[0] in (0xff01ffff, 0xff02ffff):
                    item["channel_be"] = struct.unpack_from("!H", packet, 24)[0]
                    item["channel_le"] = struct.unpack_from("<H", packet, 24)[0]
                    item["ip_matches_config"] = packet[26:46].split(b"\0", 1)[0] == config["remote_ip"].encode()
                    item["port_matches_config"] = struct.unpack_from("!H", packet, 46)[0] == 37777
                    item["endpoint_zero"] = packet[26:48] == bytes(22)
            transport_metadata.append(item)
        if packet[:2] == b"\x12\x20" and len(observed) < 40:
            opcode = struct.unpack_from("<H", packet, 2)[0]
            if opcode in (1012, 1023, 1004, 2000, 2002, 2003, 2004, 2007, 2005, 3001, 3002, 3003, 3004, 3005, 3006):
                port = struct.unpack_from("!H", ctypes.string_at(address, 4), 2)[0]
                item = {"opcode": opcode, "size": size, "port": port}
                if opcode in (2002, 2004):
                    item["seconds"] = round(time.monotonic() - started, 3)
                if opcode == 2004 and size == 8:
                    item["close_words"] = list(struct.unpack("<2I", packet))
                if opcode == 1012:
                    item["fields"] = struct.unpack_from("<3I", packet, 148)
                    item["uuid_length"] = len(packet[48:148].split(b"\0", 1)[0])
                    payload = ctypes.create_string_buffer(packet[16:48], 32)
                    decrypt(payload, 32, packet[:16])
                    item["key_matches"] = payload.raw.split(b"\0", 1)[0] == config["key"].encode()
                if opcode == 2007:
                    item["user_length"] = len(packet[4:36].split(b"\0", 1)[0])
                    item["empty_user"] = packet[4:36].split(b"\0", 1)[0] == b""
                    item["user_is_serial"] = packet[4:36].split(b"\0", 1)[0] == config.get("serial", "").upper().encode()
                if opcode == 1004:
                    item["request_fields"] = {"dest_nat": struct.unpack_from("<I", packet, 328)[0],
                                              "raw": packet[351],
                                              "local_nat": struct.unpack_from("<I", packet, 360)[0],
                                              "token_length": len(packet[468:504].split(b"\0", 1)[0]),
                                              "local_port": struct.unpack_from("<H", packet, 226)[0],
                                              "mapped_port": struct.unpack_from("<H", packet, 224)[0]}
                if opcode == 2000 and size >= 184:
                    item["route"] = struct.unpack_from("<I", packet, 120)[0]
                    item["local_id"] = struct.unpack_from("<I", packet, 4)[0]
                if opcode == 3001 and size >= 212:
                    item["local_id"] = struct.unpack_from("<I", packet, 104)[0]
                    item["remote_id"] = struct.unpack_from("<I", packet, 208)[0]
                if opcode == 2002 and size == 24:
                    sanitized = bytearray(packet)
                    for field in ("serial", "username", "password", "key", "tunnel_password"):
                        value = config.get(field, "")
                        for needle in {value.encode(), value.upper().encode(), value.lower().encode()}:
                            if not needle:
                                continue
                            at = sanitized.find(needle)
                            if at >= 0:
                                sanitized[at:at+len(needle)] = bytes(len(needle))
                    item["heartbeat_words"] = list(struct.unpack("<6I", sanitized))
                if opcode == 2005 and size >= 20:
                    item["vdt_header"] = list(struct.unpack_from("<HBBHHBBBBI", packet, 4))
                    if packet[14] == 0 and size >= 36:
                        item["control_words"] = list(struct.unpack_from("<4I", packet, 20))
                observed.append(item)
    return original_send(sock, data, size, flags, address, address_size)


protect = ctypes.WinDLL("kernel32", use_last_error=True).VirtualProtect
protect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32)]
old = ctypes.c_uint32()
assert protect(ctypes.addressof(slot), 8, 4, ctypes.byref(old))
slot.value = ctypes.cast(observe, ctypes.c_void_p).value
assert protect(ctypes.addressof(slot), 8, old.value, ctypes.byref(old))
recv_type = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_size_t, ctypes.c_void_p,
                               ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
recv_slot = ctypes.c_void_p.from_address(dll._handle + 0x40378)
original_recv = recv_type(recv_slot.value)


@recv_type
def observe_recv(sock, data, size, flags, address, address_size):
    received = original_recv(sock, data, size, flags, address, address_size)
    if received >= 4:
        packet = ctypes.string_at(data, received)
        if packet[:2] == b'\x12\x01' and received >= 16 and len(incoming) < 25:
            incoming.append({"transport_header": packet[:16].hex(), "size": received,
                             "t2u_header": packet[16:24].hex() if packet[3] == 2 else None})
        if packet[:2] == b"\x12\x20" and len(incoming) < 25:
            opcode = struct.unpack_from("<H", packet, 2)[0]
            if opcode in (1015, 1004, 1005, 2000, 2001, 3001, 3002, 3003, 2005):
                item = {"opcode": opcode, "size": received}
                if address and address_size:
                    item["port"] = struct.unpack_from("!H", ctypes.string_at(address, 4), 2)[0]
                if opcode in (1015, 1004) and received >= 208:
                    item["source_is_device"] = packet[8:108].split(b"\0", 1)[0] == config.get("serial", "").upper().encode()
                    item["dest_is_local"] = packet[108:208].split(b"\0", 1)[0] == ctypes.string_at(dll._handle + 0x54ab8)
                incoming.append(item)
    return received


assert protect(ctypes.addressof(recv_slot), 8, 4, ctypes.byref(old))
recv_slot.value = ctypes.cast(observe_recv, ctypes.c_void_p).value
assert protect(ctypes.addressof(recv_slot), 8, old.value, ctypes.byref(old))
stream_type = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_size_t, ctypes.c_void_p,
                                 ctypes.c_int, ctypes.c_int)
stream_slot = ctypes.c_void_p.from_address(dll._handle + 0x40368)
original_stream_send = stream_type(stream_slot.value)


@stream_type
def observe_stream(sock, data, size, flags):
    if size >= 16 and len(stream_metadata) < 25:
        packet = ctypes.string_at(data, size)
        stream_metadata.append({"size": size, "words": list(struct.unpack_from("<4I", packet))})
    return original_stream_send(sock, data, size, flags)


assert protect(ctypes.addressof(stream_slot), 8, 4, ctypes.byref(old))
stream_slot.value = ctypes.cast(observe_stream, ctypes.c_void_p).value
assert protect(ctypes.addressof(stream_slot), 8, old.value, ctypes.byref(old))
dll.t2u_init.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p]
dll.t2u_init.restype = None
dll.t2u_status.restype = ctypes.c_int
dll.t2u_exit.restype = None
server, key = config["server"].encode(), config["key"].encode()
dll.t2u_init(server, config["port"], key)
try:
    deadline = time.monotonic() + 15
    status = 0
    while time.monotonic() < deadline:
        status = dll.t2u_status()
        if status == 1:
            break
        time.sleep(0.1)
    peer_status = None
    proxy = None
    challenge_response = False
    if status == 1 and config.get("serial"):
        dll.t2u_add_port_v3.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                                       ctypes.c_uint16, ctypes.c_uint16]
        dll.t2u_port_status.argtypes = [ctypes.c_uint16, ctypes.c_void_p]
        local_port = dll.t2u_add_port_v3(config["serial"].upper().encode(),
                                        config["tunnel_password"].encode(),
                                        config["remote_ip"].encode(), 37777, 0)
        if 0 < local_port < 65536:
            deadline = time.monotonic() + 15
            buffer = ctypes.create_string_buffer(88)
            while time.monotonic() < deadline:
                peer_status = dll.t2u_port_status(local_port, buffer)
                if peer_status == 1:
                    proxy = struct.unpack_from("<i", buffer.raw, 24)[0]
                    break
                time.sleep(0.1)
            if peer_status == 1:
                with socket.create_connection(("127.0.0.1", local_port), timeout=3) as connection:
                    challenge = bytearray(32)
                    challenge[0:2] = b'\xa0\x01'
                    challenge[24:32] = bytes([5,2,1,1,0,0,0xa1,0xaa])
                    connection.sendall(challenge)
                    connection.settimeout(8)
                    try:
                        reply = connection.recv(4096)
                        challenge_response = bool(reply and reply[0] == 0xb0)
                    except socket.timeout:
                        pass
                    time.sleep(max(0.4, min(config.get("hold_seconds", 0), 20)))
            dll.t2u_del_port(ctypes.c_uint16(local_port))
    report = {"windows_reference_status": status, "peer_status": peer_status, "proxy": proxy,
                      "challenge_response": challenge_response,
                      "nat_type": ctypes.c_int.from_address(dll._handle + 0x57170).value,
                      "uuid_assigned": bool(ctypes.c_char.from_address(dll._handle + 0x54ab8).value),
                      "key_matches": ctypes.string_at(dll._handle + 0x57208) == key,
                      "outgoing_metadata": observed, "incoming_metadata": incoming,
                      "stream_metadata": stream_metadata, "transport_metadata": transport_metadata,
                      "ack_metadata": ack_metadata}
finally:
    dll.t2u_exit()
print(json.dumps(report))
sys.exit(0 if status == 1 else 1)
