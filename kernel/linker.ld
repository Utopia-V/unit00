OUTPUT_ARCH(riscv)
ENTRY(_start)
BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;

    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.sdata .sdata.*)
        *(.data .data.*)
    }

    .bss : {
        _bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        _bss_end = .;
    }

    . = ALIGN(4K);
    _stack_start = .;
    . = . + 64K;
    _stack_end = .;
    _kernel_end = .;
}
