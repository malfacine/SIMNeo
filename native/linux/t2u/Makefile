CC ?= cc
CFLAGS ?= -O2 -g
WARNINGS = -std=c11 -Wall -Wextra -Werror -Wpedantic
BUILD = build

.PHONY: all test sanitize clean
all: $(BUILD)/libsim_t2u_linux.so

$(BUILD):
	mkdir -p $@

$(BUILD)/libsim_t2u_linux.so: src/protocol.c src/client.c src/vdt.c include/sim_t2u.h src/protocol.h src/vdt.h | $(BUILD)
	$(CC) $(CFLAGS) $(WARNINGS) -fPIC -shared -Iinclude -o $@ src/protocol.c src/client.c src/vdt.c

$(BUILD)/test_protocol: tests/test_protocol.c src/protocol.c src/protocol.h | $(BUILD)
	$(CC) $(CFLAGS) $(WARNINGS) -Isrc -o $@ tests/test_protocol.c src/protocol.c

test: all $(BUILD)/test_protocol
	./$(BUILD)/test_protocol
	python3 -m unittest discover -s tests -p 'test_*.py' -v

sanitize:
	$(MAKE) BUILD=build-sanitize CFLAGS='-O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer' all build-sanitize/test_protocol
	./build-sanitize/test_protocol
	ASAN_OPTIONS=detect_leaks=0 LD_PRELOAD="$(shell $(CC) -print-file-name=libasan.so)" SIM_T2U_TEST_BUILD=build-sanitize python3 -m unittest discover -s tests -p 'test_*.py' -v

clean:
	rm -f $(BUILD)/libsim_t2u_linux.so $(BUILD)/test_protocol
