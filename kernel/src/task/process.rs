use crate::{
    mm::page_table::{PageTable, PhysAddr},
    task::trapframe::TrapFrame,
};

pub const USER_HEAP_START: usize = 0x2_0000;
pub const USER_HEAP_LIMIT: usize = 0x3e00_0000;

#[derive(Clone, Copy, PartialEq)]
pub enum ProcessState {
    Ready,
    Running,
    Blocked,
    Zombie(usize),
    Gone,
}

pub struct Process {
    pub pid: usize,
    pub parent_pid: usize,
    pub state: ProcessState,
    pub page_table: PageTable,
    pub trap_frame: TrapFrame,
    pub kernel_sp: usize,
    pub kernel_stack_frame: PhysAddr,
    pub heap_start: usize,
    pub heap_end: usize,
}
