// SBI ecall wrappers. Spec: RISC-V SBI v0.3+
// a7 = extension ID, a6 = function ID, a0-a3 = args, a0-a1 = return

const SBI_EXT_SRST: usize = 0x53525354;
const SBI_SRST_RESET: usize = 0;
const SBI_RESET_SHUTDOWN: usize = 0;

#[inline]
fn sbi_call(ext: usize, func: usize, arg0: usize, arg1: usize) -> (usize, usize) {
    let ret0: usize;
    let ret1: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") arg0 => ret0,
            inlateout("a1") arg1 => ret1,
            in("a6") func,
            in("a7") ext,
        );
    }
    (ret0, ret1)
}

pub fn shutdown() -> ! {
    sbi_call(SBI_EXT_SRST, SBI_SRST_RESET, SBI_RESET_SHUTDOWN, 0);
    // If SBI SRST is not supported, fallback: loop forever
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}
