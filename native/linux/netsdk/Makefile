CXX ?= c++
SDK ?= ../../../sdk/NetSDK 3.050/Linux
BUILD = build

.PHONY: all test
all: $(BUILD)/libsim_intelbras_linux.so

$(BUILD):
	mkdir -p $@

$(BUILD)/libsim_intelbras_linux.so: src/sdk.cpp include/sim_intelbras.h | $(BUILD)
	$(CXX) -std=c++17 -O2 -g -Wall -Wextra -Werror -fPIC -shared -Iinclude -isystem "$(SDK)/include" -L"$(SDK)/bin" -o $@ src/sdk.cpp -ldhnetsdk

test: all
	LD_LIBRARY_PATH="$(SDK)/bin" python3 tests/probe.py --init
