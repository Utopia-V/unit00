// trap 模块：初始化 + 处理入口

use core::arch::asm;

use crate::{console, sbi};

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
            _ => {
                console::puts("\n[TRAP]\n scause: 0x");
                print_hex(scause);
                console::puts("\n");
                sbi::shutdown();
            }
        }
    }
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
