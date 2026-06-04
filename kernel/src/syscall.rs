// syscall 分发表：从 trap_handler 中抽出，保持干净

use core::arch::asm;

use crate::{console, sbi, trap};

pub fn dispatch(syscall_no: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    match syscall_no {
        63 => {
            // read(fd, buf, len)
            let buf = arg1 as *mut u8;
            let len = arg2;
            unsafe {
                asm!("csrs sstatus, {}", in(reg) 1usize << 18);
            }
            for i in 0..len {
                unsafe { *buf.add(i) = console::read_char(); }
            }
            unsafe {
                asm!("csrc sstatus, {}", in(reg) 1usize << 18);
            }
            len // 返回实际读到的字节数
        }
        64 => {
            // write(fd, buf, len) — 忽略 fd，直接打印 buf
            let buf = arg1 as *const u8;
            let len = arg2;
            unsafe {
                asm!("csrs sstatus, {}", in(reg) 1usize << 18);
            }
            for i in 0..len {
                console::putchar(unsafe { *buf.add(i) });
            }
            unsafe {
                asm!("csrc sstatus, {}", in(reg) 1usize << 18);
            }
            len
        }
        93 => {
            // exit(code)
            console::puts("\nuser exit, code=");
            trap::print_hex(arg0);
            console::puts("\n");
            sbi::shutdown();
        }
        172 => {
            // getpid() 暂时实现
            1
        }
        _ => {
            console::puts("\nunknown syscall: ");
            trap::print_hex(syscall_no);
            console::puts("\n");
            sbi::shutdown();
        }
    }
}
