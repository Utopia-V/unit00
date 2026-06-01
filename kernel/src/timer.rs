const SBI_EXT_TIME: usize = 0x54494D45;
const SBI_SET_TIMER: usize = 0;

pub fn set_timer(stime_value: usize) {
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a0") stime_value,
            in("a6") SBI_SET_TIMER,
            in("a7") SBI_EXT_TIME,
        );
    }
}

pub fn read_time() -> usize {
    let t: usize;
    unsafe {
        core::arch::asm!("csrr {}, time", out(reg) t);
    }
    t
}
