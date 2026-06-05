#[derive(Clone, Copy)]
pub struct TrapFrame {
    pub ra: usize,
    pub sp: usize,
    pub a0: usize,
    pub a1: usize,
    pub a2: usize,
    pub a7: usize,
    pub scause: usize,
    pub sepc: usize,
    pub sstatus: usize,
}