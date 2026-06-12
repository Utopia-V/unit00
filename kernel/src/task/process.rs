use crate::{
    mm::page_table::{PageTable, PhysAddr},
    task::trapframe::TrapFrame,
};

pub const USER_HEAP_START: usize = 0x2_0000;
pub const USER_HEAP_LIMIT: usize = 0x3e00_0000;
pub const USER_MMAP_START: usize = USER_HEAP_LIMIT;
pub const USER_MMAP_LIMIT: usize = 0x3f00_0000;
pub const MAX_MMAP_AREAS: usize = 32;
pub const KERNEL_STACK_PAGES: usize = 8;
pub const KERNEL_STACK_SIZE: usize = KERNEL_STACK_PAGES * 4096;

#[derive(Clone, Copy)]
pub struct MmapArea {
    pub start: usize,
    pub len: usize,
    pub prot: usize,
    pub flags: usize,
    pub used: bool,
}

impl MmapArea {
    pub const EMPTY: Self = Self {
        start: 0,
        len: 0,
        prot: 0,
        flags: 0,
        used: false,
    };

    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

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
    pub mmap_areas: [MmapArea; MAX_MMAP_AREAS],
}
