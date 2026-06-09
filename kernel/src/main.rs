#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod console;
mod mm;
mod sbi;
mod syscall;
mod task;
mod timer;
mod trap;

use crate::mm::frame::{alloc_frame, frame_init};
use crate::mm::page_table::{PTEFlags, PhysAddr, VirtAddr, init_kernel};
use crate::timer::{read_time, set_timer};
use core::arch::{asm, global_asm};
use core::panic::PanicInfo;

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
    csrw sscratch, zero
    tail rust_main
    "
);

global_asm!(
    "
    .section .text.trap_entry
    .global trap_entry
    .global trap_exit_restore
    .align 4
trap_entry:
    // sscratch != 0 means U-mode was running and sscratch holds kernel_sp.
    // sscratch == 0 means the trap came from S-mode; this stage treats it as fatal.
    csrrw t0, sscratch, t0
    bnez  t0, 1f
    csrrw t0, sscratch, t0
    call  kernel_trap_handler

1:
    addi t0, t0, -288

    sd   zero,   0(t0)
    sd   ra,     8(t0)
    sd   sp,    16(t0)
    sd   gp,    24(t0)
    sd   tp,    32(t0)
    sd   t1,    48(t0)
    csrr t1, sscratch
    sd   t1,    40(t0)
    sd   t2,    56(t0)
    sd   s0,    64(t0)
    sd   s1,    72(t0)
    sd   a0,    80(t0)
    sd   a1,    88(t0)
    sd   a2,    96(t0)
    sd   a3,   104(t0)
    sd   a4,   112(t0)
    sd   a5,   120(t0)
    sd   a6,   128(t0)
    sd   a7,   136(t0)
    sd   s2,   144(t0)
    sd   s3,   152(t0)
    sd   s4,   160(t0)
    sd   s5,   168(t0)
    sd   s6,   176(t0)
    sd   s7,   184(t0)
    sd   s8,   192(t0)
    sd   s9,   200(t0)
    sd   s10,  208(t0)
    sd   s11,  216(t0)
    sd   t3,   224(t0)
    sd   t4,   232(t0)
    sd   t5,   240(t0)
    sd   t6,   248(t0)

    csrr t1, sstatus
    sd   t1,   256(t0)
    csrr t1, sepc
    sd   t1,   264(t0)
    csrr t1, scause
    sd   t1,   272(t0)
    csrr t1, stval
    sd   t1,   280(t0)

    mv   sp, t0
    csrw sscratch, zero

    ld   a0, 272(sp)    // scause
    ld   a1, 264(sp)    // sepc
    ld   a2,  80(sp)    // user a0
    ld   a3,  88(sp)    // user a1
    ld   a4,  96(sp)    // user a2
    ld   a5, 136(sp)    // user a7
    ld   a6, 280(sp)    // stval

    call trap_handler

    // U-mode ecall? → save return value + sepc += 4
    ld   t0, 272(sp)    // scause
    li   t1, 8
    bne  t0, t1, 1f
    sd   a0,  80(sp)    // dispatch 返回值 → 用户 a0
    ld   t0, 264(sp)
    addi t0, t0, 4
    sd   t0, 264(sp)
1:
    j trap_exit_restore

    // trap_exit_restore: schedule()/init 入口
trap_exit_restore:
    ld   t0, 264(sp)
    csrw sepc, t0
    ld   t0, 256(sp)
    csrw sstatus, t0
    ld   t0,  16(sp)
    csrw sscratch, t0

    ld   ra,     8(sp)
    ld   gp,    24(sp)
    ld   tp,    32(sp)
    ld   t1,    48(sp)
    ld   t2,    56(sp)
    ld   s0,    64(sp)
    ld   s1,    72(sp)
    ld   a0,    80(sp)
    ld   a1,    88(sp)
    ld   a2,    96(sp)
    ld   a3,   104(sp)
    ld   a4,   112(sp)
    ld   a5,   120(sp)
    ld   a6,   128(sp)
    ld   a7,   136(sp)
    ld   s2,   144(sp)
    ld   s3,   152(sp)
    ld   s4,   160(sp)
    ld   s5,   168(sp)
    ld   s6,   176(sp)
    ld   s7,   184(sp)
    ld   s8,   192(sp)
    ld   s9,   200(sp)
    ld   s10,  208(sp)
    ld   s11,  216(sp)
    ld   t3,   224(sp)
    ld   t4,   232(sp)
    ld   t5,   240(sp)
    ld   t6,   248(sp)
    ld   t0,    40(sp)
    addi sp, sp, 288
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
    // fork()
    li   a7, 220
    ecall
    // a0 != 0 -> parent, a0 == 0 -> child
    beqz a0, child

parent:
    // wait(&exit_code) - store ptr on stack
    addi sp, sp, -16
    li   a7, 260
    mv   a0, sp       // exit_code_ptr = sp
    ecall
    // exit(0)
    li   a7, 93
    li   a0, 0
    ecall

child:
    // exit(42)
    li   a7, 93
    li   a0, 42
    ecall
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
    frame_init(kernel_end);
    let (mut pt, satp) = init_kernel(
        PhysAddr(crate::mm::frame::RAM_BASE),
        kernel_end,
        alloc_frame,
    );

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
        PTEFlags::new(true, false, true, true), // R+X, U=1
    );

    // 用户栈页：映射到 0x3F00_0000（VPN[2]=0，< KERNEL_VPN2_MIN）
    let stack_page = alloc_frame().expect("no frame for user stack");
    pt.map(
        VirtAddr(0x3F00_0000),
        stack_page,
        PTEFlags::new(true, true, false, true), // R+W, U=1
    );

    unsafe {
        asm!("csrw satp, {}", in(reg) satp);
        asm!("sfence.vma");
    }

    trap::init(trap_entry as *const () as usize);

    // 传递 trap_exit_restore 地址给调度器
    unsafe {
        crate::trap::TRAP_EXIT_RESTORE_ADDR = trap_exit_restore as *const () as usize;
    }

    // sscratch=0 表示当前在内核态；用户态运行时由 trap_exit_restore 写入 kernel_sp。
    unsafe {
        asm!("csrw sscratch, zero");
    }
    let next = read_time() + 10_000_000;
    set_timer(next);
    // Enable supervisor timer interrupts at the source, but keep SIE clear
    // while still in the kernel. The initial TrapFrame sets SPIE so sret
    // enables interrupts only after entering user mode.
    unsafe {
        asm!("csrs sie, {}", in(reg) 1usize << 5);
    }

    // ── 构造 init Process ──
    use crate::task::process::{Process, ProcessState};
    use crate::task::trapframe::TrapFrame;

    let kernel_stack = alloc_frame().expect("no frame for init kernel stack");
    let kernel_sp = kernel_stack.0 + 4096;
    let init_satp = pt.satp_val();

    let init = Process {
        pid: crate::task::scheduler::alloc_pid(),
        parent_pid: 0, // 哨兵，无父进程
        state: ProcessState::Running,
        page_table: pt,
        trap_frame: TrapFrame::new_user(0x10000, 0x3F001000),
        kernel_sp,
        kernel_stack_frame: kernel_stack,
    };

    unsafe {
        crate::task::scheduler::PROCESS_LIST[0] = Some(init);
        crate::task::scheduler::CURRENT = 0;
    }

    // 手工搭栈 → 跳 trap_exit_restore
    let init = crate::task::scheduler::current();
    unsafe {
        asm!("csrw satp, {}", in(reg) init_satp);
    }
    unsafe {
        asm!("sfence.vma");
    }
    let new_sp = init.kernel_sp - TrapFrame::SIZE;
    unsafe {
        init.trap_frame.write_to_stack(new_sp);
    }
    unsafe {
        asm!("mv sp, {}", in(reg) new_sp);
    }
    let addr = unsafe { crate::trap::TRAP_EXIT_RESTORE_ADDR };
    unsafe {
        asm!("jr {}", in(reg) addr, options(noreturn));
    }
}

unsafe extern "C" {
    fn trap_entry();
    fn trap_exit_restore();
    fn _kernel_end();
    fn user_prog_start();
    fn user_prog_end();
}
