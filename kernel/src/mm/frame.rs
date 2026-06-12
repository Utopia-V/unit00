use core::{
    ptr::read,
    sync::atomic::{AtomicU32, AtomicU64},
};

use crate::mm::page_table::PhysAddr;

pub const RAM_BASE: usize = 0x8000_0000; // QEMU virt 物理内存起点
pub const RAM_SIZE: usize = 128 * 1024 * 1024; // 128MB，后续从 FDT 探测

static mut NEXT_FRAME: usize = 0; // 在 rust_main 里根据 kernel_end 初始化
static mut FRAME_REFS: [AtomicU32; 32768] = unsafe { core::mem::zeroed() };
static FREE_LIST_HEAD: AtomicU64 = AtomicU64::new(0);

fn frame_index(pa: PhysAddr) -> usize {
    if !is_managed(pa) {
        crate::console::puts("\n[FRAME] bad pa=0x");
        crate::trap::print_hex(pa.0);
        crate::console::puts("\n");
        panic!("frame_index: physical address out of managed RAM");
    }
    (pa.0 - RAM_BASE) >> 12
}

fn is_managed(pa: PhysAddr) -> bool {
    pa.0 >= RAM_BASE && pa.0 < RAM_BASE + RAM_SIZE
}

pub fn inc_ref(pa: PhysAddr) {
    let idx = frame_index(pa);
    unsafe {
        FRAME_REFS[idx].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

pub fn free_frame(pa: PhysAddr) {
    let head = FREE_LIST_HEAD.load(core::sync::atomic::Ordering::Relaxed);
    unsafe {
        core::ptr::write(pa.0 as *mut u64, head);
    }
    FREE_LIST_HEAD.store(pa.0 as u64, core::sync::atomic::Ordering::Relaxed);
    let idx = frame_index(pa);
    unsafe { FRAME_REFS[idx].store(0, core::sync::atomic::Ordering::Relaxed) };
}

pub fn dec_ref(pa: PhysAddr) -> u32 {
    let idx = frame_index(pa);
    let prev = unsafe { FRAME_REFS[idx].fetch_sub(1, core::sync::atomic::Ordering::Relaxed) };
    if prev == 1 {
        free_frame(pa);
    }
    prev - 1
}

pub fn get_ref(pa: PhysAddr) -> u32 {
    let idx = frame_index(pa);
    unsafe { FRAME_REFS[idx].load(core::sync::atomic::Ordering::Relaxed) }
}

pub(crate) fn frame_init(kernel_end: PhysAddr) {
    unsafe {
        NEXT_FRAME = (kernel_end.0 + 4095) & !4095;
    }
}

pub fn alloc_frame() -> Option<PhysAddr> {
    let head = FREE_LIST_HEAD.load(core::sync::atomic::Ordering::Relaxed);
    if head != 0 {
        let next = unsafe { read(head as *const u64) };
        FREE_LIST_HEAD.store(next, core::sync::atomic::Ordering::Relaxed);
        let pa = PhysAddr(head as usize);
        let idx = frame_index(pa);
        unsafe { FRAME_REFS[idx].store(1, core::sync::atomic::Ordering::Relaxed) };
        Some(pa)
    } else {
        let p = unsafe { NEXT_FRAME };
        if p >= RAM_BASE + RAM_SIZE {
            return None;
        }
        unsafe { NEXT_FRAME += 4096 };
        let pa = PhysAddr(p);
        let idx = frame_index(pa);
        unsafe {
            FRAME_REFS[idx].store(1, core::sync::atomic::Ordering::Relaxed);
        }
        Some(pa)
    }
}

pub fn alloc_contiguous_frames(pages: usize) -> Option<PhysAddr> {
    if pages == 0 {
        return None;
    }
    let bytes = pages.checked_mul(4096)?;
    let start = unsafe { NEXT_FRAME };
    let end = start.checked_add(bytes)?;
    if end > RAM_BASE + RAM_SIZE {
        return None;
    }

    unsafe {
        NEXT_FRAME = end;
    }
    for i in 0..pages {
        let pa = PhysAddr(start + i * 4096);
        let idx = frame_index(pa);
        unsafe {
            FRAME_REFS[idx].store(1, core::sync::atomic::Ordering::Relaxed);
        }
    }
    Some(PhysAddr(start))
}
