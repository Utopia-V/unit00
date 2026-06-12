// trap 模块：初始化 + 处理入口

use core::arch::asm;

use crate::mm::page_table::{PTEFlags, PTEntry, VirtAddr};
use crate::task::process::ProcessState;
use crate::task::scheduler::{current, reparent_orphans_to_init, schedule, wake_parent};
use crate::task::trapframe::TrapFrame;
use crate::{console, sbi};

/// trap_exit_restore 汇编标签的地址，由 rust_main 在 init 时填写
pub static mut TRAP_EXIT_RESTORE_ADDR: usize = 0;

pub fn init(addr: usize) {
    unsafe {
        asm!("csrw stvec, {}", in(reg) addr);
    }
}

#[unsafe(no_mangle)]
extern "C" fn kernel_trap_handler() -> ! {
    let scause: usize;
    let sepc: usize;
    let stval: usize;
    let sstatus: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) scause);
        asm!("csrr {}, sepc", out(reg) sepc);
        asm!("csrr {}, stval", out(reg) stval);
        asm!("csrr {}, sstatus", out(reg) sstatus);
    }
    console::puts("\n[KERNEL TRAP]\n");
    console::puts(" scause=0x");
    print_hex(scause);
    console::puts(" sepc=0x");
    print_hex(sepc);
    console::puts(" stval=0x");
    print_hex(stval);
    console::puts(" sstatus=0x");
    print_hex(sstatus);
    console::puts("\n");
    sbi::shutdown();
}

#[allow(unused)]
pub fn trigger_breakpoint() {
    unsafe {
        asm!("ebreak");
    }
}

#[unsafe(no_mangle)]
extern "C" fn trap_handler(
    scause: usize,
    _sepc: usize,
    _arg0: usize,
    _arg1: usize,
    _arg2: usize,
    _syscall_no: usize,
    stval: usize,
) -> usize {
    // 更新当前进程 TrapFrame，使 syscall（如 fork）能读到正确的寄存器值。
    // trap frame 位于 kernel_sp - TrapFrame::SIZE，由 trap_entry 汇编填写。
    {
        let proc = current();
        let frame_base = proc.kernel_sp - TrapFrame::SIZE;
        unsafe {
            proc.trap_frame = TrapFrame::read_from_stack(frame_base);
        }
    }

    if scause >> 63 == 1 {
        // 中断（timer）
        static mut TICK_COUNT: usize = 0;
        unsafe {
            TICK_COUNT += 1;
        }
        console::puts("tick ");
        print_hex(unsafe { TICK_COUNT });
        console::puts("\n");
        let next = crate::timer::read_time() + 10_000_000;
        crate::timer::set_timer(next);
        0 // 中断不设用户返回值
    } else {
        // 异常
        match scause {
            8 => {
                let tf = current().trap_frame;
                crate::syscall::dispatch(
                    tf.regs[17],
                    [
                        tf.regs[10],
                        tf.regs[11],
                        tf.regs[12],
                        tf.regs[13],
                        tf.regs[14],
                        tf.regs[15],
                    ],
                )
            }
            12 | 13 | 15 => page_fault_handler(scause, stval),
            _ => {
                console::puts("\n[TRAP]\n scause: 0x");
                print_hex(scause);
                console::puts("\n");
                sbi::shutdown();
            }
        }
    }
}

// ── Page Fault Handler ──

fn page_fault_handler(scause: usize, stval: usize) -> usize {
    // 非 Store Page Fault (15) → 段错误
    if scause != 15 {
        do_segfault(scause, stval);
        return 0;
    }

    // Store Page Fault: 查 PTE，判断 COW
    let vaddr = VirtAddr(stval);
    let proc = crate::task::scheduler::current();
    if let Some(entry) = proc.page_table.lookup(vaddr)
        && entry.is_cow()
        && entry.is_r()
        && !entry.is_w()
        && entry.is_u()
    {
        handle_cow_fault(vaddr, entry);
        return 0;
    }

    // 非 COW 页面 → 真段错误
    do_segfault(scause, stval);
    0
}

pub(crate) fn handle_cow_fault(_vaddr: VirtAddr, entry: &mut PTEntry) {
    use crate::mm::frame;
    let ppn = entry.ppn_to_addr();
    let refcount = frame::get_ref(ppn);

    if refcount > 1 {
        // 多进程共享 → 分配新帧 + 拷贝
        let new_frame = frame::alloc_frame().expect("COW: alloc_frame failed");
        unsafe {
            core::ptr::copy_nonoverlapping(ppn.0 as *const u8, new_frame.0 as *mut u8, 4096);
        }
        frame::dec_ref(ppn);
        *entry = PTEntry::new_leaf(new_frame, {
            let f = entry.flags();
            PTEFlags::new(true, true, f.is_x(), true) // R+W, U=1, preserve X
        });
        entry.clear_cow();
    } else {
        // 唯一进程 → 原地恢复写权限
        entry.set_w(true);
        entry.clear_cow();
    }
    unsafe { asm!("sfence.vma") };
}

fn do_segfault(scause: usize, stval: usize) {
    console::puts("\n[SEGFAULT] scause=0x");
    print_hex(scause);
    console::puts(" stval=0x");
    print_hex(stval);
    console::puts("\n");

    let cur = current();
    reparent_orphans_to_init();
    cur.state = ProcessState::Zombie(1);
    wake_parent();
    schedule();
}

pub(crate) fn print_hex(mut val: usize) {
    for _ in 0..16 {
        let nibbel = ((val >> 60) & 0xF) as u8;
        let c = if nibbel < 10 {
            b'0' + nibbel
        } else {
            b'a' + nibbel - 10
        };
        console::putchar(c);
        val <<= 4;
    }
}
