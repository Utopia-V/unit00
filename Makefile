ARCH ?= riscv64gc-unknown-none-elf
MODE ?= debug
KERNEL_BIN = target/$(ARCH)/$(MODE)/unit00

ifeq ($(MODE),release)
    CARGO_FLAGS = --release
endif

.PHONY: all build run check clippy fmt fmt-check clean qemu

# 评测系统入口：必须产出 kernel-rv
all: kernel-rv

kernel-rv: build
	cp $(KERNEL_BIN) kernel-rv

# 还原 .cargo（评测系统过滤隐藏目录，仓库里以 cargo_hidden 提交）
.cargo: cargo_hidden
	cp -r cargo_hidden .cargo

build: .cargo
	cargo build $(CARGO_FLAGS)

run: build
	qemu-system-riscv64 \
		-machine virt \
		-m 128M \
		-nographic \
		-bios default \
		-kernel $(KERNEL_BIN)

check: .cargo
	cargo check

clippy: .cargo
	cargo clippy $(CARGO_FLAGS) -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean
	rm -f kernel-rv

qemu: run
