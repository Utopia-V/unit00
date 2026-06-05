// trap 模块：初始化 + 处理入口

use core::arch::asm;

use crate::mm::page_table::{PTEntry, VirtAddr, PTEFlags};
use crate::task::process::ProcessState;
use crate::task::scheduler::{current, reparent_orphans_to_init, schedule, wake_parent};
use crate::{console, sbi};

/// trap_exit_restore 汇编标签的地址，由 rust_main 在 init 时填写
pub static mut TRAP_EXIT_RESTORE_ADDR: usize = 0;

pub fn init(addr: usize) {
    unsafe {
        asm!("csrw stvec, {}", in(reg) addr);
    }
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
    arg0: usize,
    arg1: usize,
    arg2: usize,
    syscall_no: usize,
    stval: usize,
) -> usize {
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
            8 => crate::syscall::dispatch(syscall_no, arg0, arg1, arg2),
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
        // do_segfault 调 schedule() 所以不可达，编译器需要返回值
        return 0;
    }

    // Store Page Fault: 查 PTE，判断 COW
    let vaddr = VirtAddr(stval);
    let proc = crate::task::scheduler::current();
    if let Some(entry) = proc.page_table.lookup(vaddr) {
        if entry.is_cow() && entry.is_r() && !entry.is_w() && entry.is_u() {
            // COW 共享页 → 拆分
            handle_cow_fault(vaddr, entry);
            return 0; // sret 重试写操作
        }
    }

    // 非 COW 页面 → 真段错误
    do_segfault(scause, stval);
    0
}

fn handle_cow_fault(_vaddr: VirtAddr, entry: &mut PTEntry) {
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
