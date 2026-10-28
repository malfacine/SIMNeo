#include "protocol.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    char digest[33];
    md5_hex("", digest);
    assert(strcmp(digest, "d41d8cd98f00b204e9800998ecf8427e") == 0);
    md5_hex("abc", digest);
    assert(strcmp(digest, "900150983cd24fb0d6963f7d28e17f72") == 0);
    md5_hex("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", digest);
    assert(strcmp(digest, "d174ab98d277d9f5a5611c2c9f419d9f") == 0);
    unsigned char block[8] = {0}, key[16] = {0};
    /* TEA's zero-key, zero-plaintext vector, encoded little endian. */
    const unsigned char expected[8] = {0x0a,0x3a,0xea,0x41,0x40,0xa9,0xba,0x94};
    tea_encrypt(block, sizeof block, key);
    assert(memcmp(block, expected, sizeof block) == 0);
    tea_decrypt(block, sizeof block, key);
    const unsigned char zero[8] = {0};
    assert(memcmp(block, zero, sizeof block) == 0);

    unsigned char packet[2048], decoded[2048];
    size_t n = signal_packet(packet, sizeof packet, 1010, 123, 456, "SERIAL");
    assert(n == 28);
    assert(read_le32(packet) == 0x03f22012);
    assert(read_le16(packet + 16) == 7);
    assert(read_le16(packet + 18) == 8);
    assert(signal_payload(packet, n, decoded, sizeof decoded) == 7);
    assert(strcmp((char *)decoded, "SERIAL") == 0);
    assert(signal_payload(packet, n - 1, decoded, sizeof decoded) == -1);
    packet[18] = 7;
    assert(signal_payload(packet, n, decoded, sizeof decoded) == -1);
    assert(signal_packet(packet, 24, 1010, 1, 2, "SERIAL") == 0);
    for (size_t i = 0; i < 20; ++i)
        assert(signal_payload(packet, i, decoded, sizeof decoded) == -1);

    n = discovery_packet(packet, sizeof packet, 123);
    assert(n == 16);
    assert(read_le32(packet) == 0x13892012);
    assert(read_le32(packet + 4) == 123);
    assert(read_le32(packet + 8) == 0x00320001);
    assert(read_le32(packet + 12) == 1);
    puts("protocol: TEA vector, encrypted framing, bounds and discovery passed");
    return 0;
}
