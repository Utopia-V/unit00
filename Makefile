ARCH ?= riscv64gc-unknown-none-elf
MODE ?= debug
KERNEL_BIN = target/$(ARCH)/$(MODE)/unit00

ifeq ($(MODE),release)
    CARGO_FLAGS = --release
endif

.PHONY: build run check clippy fmt clean qemu

build:
	cargo build $(CARGO_FLAGS)

run: build
	qemu-system-riscv64 \
		-machine virt \
		-m 128M \
		-nographic \
		-bios default \
		-kernel $(KERNEL_BIN)

check:
	cargo check

clippy:
	cargo clippy $(CARGO_FLAGS) -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean

qemu: run
