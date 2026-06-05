use crate::{mm::page_table::{PageTable, PhysAddr}, task::trapframe::TrapFrame};

#[derive(Clone, PartialEq)]
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
}