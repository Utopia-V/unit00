#![no_std]
#![no_main]

mod console;
mod mm;
mod sbi;
mod timer;
mod trap;

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use crate::mm::page_table::{PhysAddr, VirtAddr, PTEFlags, init_kernel};
use crate::timer::{read_time, set_timer};

static mut NEXT_FRAME: usize = 0; // 在 rust_main 里根据 kernel_end 初始化

global_asm!(
    "
    .section .text.entry
    .globl _start
_start:
    // Zero BSS
    la t0, _bss_start
    la t1, _bss_end
1:
    bge t0, t1, 2f
    sd  zero, 0(t0)
    addi t0, t0, 8
    j   1b
2:
    la   sp, _stack_end
    la   t0, _stack_start
    csrw sscratch, t0
    tail rust_main
    "
);

global_asm!(
    "
    .section .text.trap_entry
    .global trap_entry
    .align 4
trap_entry:
    csrrw sp, sscratch, sp
    addi sp, sp, -56
    sd   ra,  0(sp)
    sd   a0,  8(sp)    // 用户 a0
    sd   a1, 16(sp)    // 用户 a1
    sd   a2, 24(sp)    // 用户 a2
    sd   a7, 32(sp)    // 用户 a7

    csrr t0, scause
    csrr t1, sepc
    sd   t0, 40(sp)    // 保存 scause
    sd   t1, 48(sp)    // 保存 sepc
    mv   a0, t0
    mv   a1, t1
    ld   a2,  8(sp)    // 用户 a0 → arg2
    ld   a3, 16(sp)    // 用户 a1 → arg3
    ld   a4, 24(sp)    // 用户 a2 → arg4
    ld   a5, 32(sp)    // 用户 a7 → arg5

    call trap_handler

    // U-mode ecall? → sepc += 4
    ld   t0, 40(sp)    // scause
    li   t1, 8
    bne  t0, t1, 1f
    ld   t0, 48(sp)
    addi t0, t0, 4
    sd   t0, 48(sp)
1:
    ld   t0, 48(sp)
    csrw sepc, t0

    ld   a7, 32(sp)
    ld   a2, 24(sp)
    ld   a1, 16(sp)
    ld   a0,  8(sp)
    ld   ra,  0(sp)
    addi sp, sp, 56
    csrrw sp, sscratch, sp
    sret
    "
);

global_asm!(
    "
    .section .rodata.user_prog
    .global user_prog_start
    .global user_prog_end
user_prog_start:
    addi sp, sp, -16
    li   t0, 0x20696D616E617961
    sd   t0, 0(sp)
    li   t0, 0x696572
    sw   t0, 8(sp)

    li   a7, 64
    li   a0, 1
    mv   a1, sp
    li   a2, 11
    ecall
    addi sp, sp, 16
    
1:
    j    1b
msg:
    .ascii \"Hello, world!\"
user_prog_end:
    "
);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    console::puts("\n[PANIC]");
    if let Some(loc) = info.location() {
        console::puts(" at ");
        console::puts(loc.file());
    }
    console::puts("\n");
    sbi::shutdown();
}

#[unsafe(no_mangle)]
extern "C" fn rust_main() -> ! {
    console::puts("Unit00 booting...\n");

    let kernel_end = PhysAddr(_kernel_end as *const () as usize);
    // 帧分配器从 kernel_end 向上对齐到 4K 之后开始
    unsafe { NEXT_FRAME = (kernel_end.0 + 4095) & !4095; }
    let (mut pt, satp) = init_kernel(PhysAddr(0x8000_0000), kernel_end, alloc_frame);

    // UART MMIO 区域
    pt.map(
        VirtAddr(0x1000_0000),
        PhysAddr(0x1000_0000),
        PTEFlags::new(true, true, false, false),
    );

    // 用户代码页：分配物理页、拷贝程序、映射到 0x1_0000
    let code_page = alloc_frame().expect("no frame for user code");
    let prog_start = user_prog_start as *const () as usize;
    let prog_end = user_prog_end as *const () as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(
            prog_start as *const u8,
            code_page.0 as *mut u8,
            prog_end - prog_start,
        );
    }
    pt.map(
        VirtAddr(0x1_0000),
        code_page,
        PTEFlags::new(true, false, true, true),  // R+X, U=1
    );

    // 用户栈页：映射到 0x1_0000_0000
    let stack_page = alloc_frame().expect("no frame for user stack");
    pt.map(
        VirtAddr(0x1_0000_0000),
        stack_page,
        PTEFlags::new(true, true, false, true),  // R+W, U=1
    );

    unsafe {
        asm!("csrw satp, {}", in(reg) satp);
        asm!("sfence.vma");
    }

    trap::init(trap_entry as *const () as usize);

    // sscratch 设内核 sp——timer 中断在内核态触发时靠它换回正确的栈
    unsafe { asm!("csrw sscratch, sp"); }
    let next = read_time() + 10_000_000;
    set_timer(next);
    unsafe {
        asm!("csrs sie, {}", in(reg) 1usize << 5);
        asm!("csrs sstatus, {}", in(reg) 1usize << 1);
    }

    // 切到用户态
    unsafe {
        asm!(
            "csrw sscratch, sp",
            "li   sp, 0x100001000",
            "li   t0, 0x10000",
            "csrw sepc, t0",
            "csrw sstatus, zero",
            "sret",
            options(noreturn),
        );
    }
}

unsafe extern "C" {
    fn trap_entry();
    fn _kernel_end();
    fn user_prog_start();
    fn user_prog_end();
}

fn alloc_frame() -> Option<mm::page_table::PhysAddr> {
    let p = unsafe { NEXT_FRAME };
    unsafe { NEXT_FRAME += 4096 };
    Some(mm::page_table::PhysAddr(p))
}