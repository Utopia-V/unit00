// syscall 分发表：从 trap_handler 中抽出，保持干净

use core::arch::asm;

use crate::{
    console,
    mm::{
        frame::{self, alloc_frame, dec_ref},
        page_table::{KERNEL_VPN2_MIN, PTEntry, PageTable, VirtAddr},
    },
    sbi,
    task::{
        process::{Process, ProcessState},
        scheduler::{alloc_pid, current, find_empty_slot, reparent_orphans_to_init, schedule, wake_parent},
    },
    trap,
};

pub fn dispatch(syscall_no: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    match syscall_no {
        63 => {
            let buf = arg1 as *mut u8;
            let len = arg2;
            unsafe { asm!("csrs sstatus, {}", in(reg) 1usize << 18); }
            for i in 0..len {
                unsafe { *buf.add(i) = console::read_char(); }
            }
            unsafe { asm!("csrc sstatus, {}", in(reg) 1usize << 18); }
            len
        }
        64 => {
            let buf = arg1 as *const u8;
            let len = arg2;
            unsafe { asm!("csrs sstatus, {}", in(reg) 1usize << 18); }
            for i in 0..len {
                console::putchar(unsafe { *buf.add(i) });
            }
            unsafe { asm!("csrc sstatus, {}", in(reg) 1usize << 18); }
            len
        }
        93 => {
            reparent_orphans_to_init();

            current().page_table.for_each_leaf(0, KERNEL_VPN2_MIN, &mut |_vaddr, entry: &mut PTEntry| {
                if !entry.flags().is_u() {
                    return;
                }
                let pa = entry.ppn_to_addr();
                dec_ref(pa);
                *entry = PTEntry::empty();
            });
            unsafe { asm!("sfence.vma"); }

            current().state = crate::task::process::ProcessState::Zombie(arg0);
            wake_parent();
            schedule()
        }
        172 => {
            1
        }
        220 => sys_fork(),
        221 => {
            // stub: 文件系统就绪后实现 ELF 加载
            let cur = current();
            cur.trap_frame.sepc = 0x10000;
            cur.trap_frame.set_sp(0x3F001000);
            0
        }
        260 => {
            sys_wait(arg0 as *mut usize)
        }
        _ => {
            console::puts("\nunknown syscall: ");
            trap::print_hex(syscall_no);
            console::puts("\n");
            sbi::shutdown();
        }
    }
}

fn sys_fork() -> usize {
    let idx = match find_empty_slot() {
        Some(i) => i,
        None => return (-11isize) as usize,
    };
    let pid = alloc_pid();

    let root_frame = alloc_frame().expect("fork: no frame for root page table");
    unsafe { core::ptr::write_bytes(root_frame.0 as *mut u8, 0, 4096); }

    let parent_root = current().page_table.root_addr().0 as *const u64;
    let child_root = root_frame.0 as *mut u64;
    for vpn2 in KERNEL_VPN2_MIN..512 {
        let pte = unsafe { core::ptr::read(parent_root.add(vpn2)) };
        if pte != 0 {
            unsafe { core::ptr::write(child_root.add(vpn2), pte); }
        }
    }

    let kernel_stack = alloc_frame().expect("fork: no frame for kernel stack");
    let kernel_sp = kernel_stack.0 + 4096;

    let mut child_tf = current().trap_frame;
    child_tf.set_a0(0);
    child_tf.sepc += 4;
    child_tf.scause = 8;

    let parent_ret = pid;

    let mut child_pt = PageTable::new(root_frame, alloc_frame);

    current().page_table.for_each_leaf(0, KERNEL_VPN2_MIN, &mut |vaddr, entry: &mut PTEntry| {
        if !entry.is_u() {
            return;
        }
        let pa = entry.ppn_to_addr();
        frame::inc_ref(pa);
        if entry.is_w() {
            entry.clear_w();
            entry.set_cow();
        }
        child_pt.map(vaddr, pa, entry.flags());
    });

    current().page_table.for_each_leaf(0, KERNEL_VPN2_MIN, &mut |vaddr, entry: &mut PTEntry| {
        if entry.is_u() {
            return;
        }
        child_pt.map(vaddr, entry.ppn_to_addr(), entry.flags());
    });

    let child = Process {
        pid,
        parent_pid: current().pid,
        state: ProcessState::Ready,
        page_table: child_pt,
        trap_frame: child_tf,
        kernel_sp,
        kernel_stack_frame: kernel_stack,
    };

    unsafe {
        crate::task::scheduler::PROCESS_LIST[idx] = Some(child);
    }

    unsafe { asm!("sfence.vma"); }

    parent_ret
}

fn sys_wait(exit_code_ptr: *mut usize) -> usize {
    use crate::task::scheduler::{PROCESS_LIST, MAX_PROCESSES};

    let cur_pid = current().pid;

    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &mut PROCESS_LIST[i] };
        if let Some(child) = slot {
            if child.parent_pid == cur_pid {
                if let ProcessState::Zombie(code) = child.state {
                    if !exit_code_ptr.is_null() {
                        // 主动处理 COW：防止嵌套 page fault
                        let vaddr = VirtAddr(exit_code_ptr as usize);
                        if let Some(entry) = current().page_table.lookup(vaddr) {
                            if entry.is_cow() && entry.is_r() && !entry.is_w() && entry.is_u() {
                                trap::handle_cow_fault(vaddr, entry);
                            }
                        }
                        unsafe {
                            asm!("csrs sstatus, {}", in(reg) 1usize << 18);
                            core::ptr::write(exit_code_ptr, code);
                            asm!("csrc sstatus, {}", in(reg) 1usize << 18);
                        }
                    }

                    free_user_pt_resources(child);

                    child.state = ProcessState::Gone;
                    return child.pid;
                }
            }
        }
    }

    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &PROCESS_LIST[i] };
        if let Some(child) = slot {
            if child.parent_pid == cur_pid
                && child.state != ProcessState::Gone
                && !matches!(child.state, ProcessState::Zombie(_))
            {
                current().state = ProcessState::Blocked;
                schedule();
            }
        }
    }

    (-10isize) as usize
}

fn free_user_pt_resources(child: &mut Process) {
    use crate::mm::frame;

    frame::free_frame(child.kernel_stack_frame);

    let root_addr = child.page_table.root_addr();
    free_intermediate_frames(root_addr, 0, KERNEL_VPN2_MIN);

    frame::free_frame(root_addr);
}

fn free_intermediate_frames(root: crate::mm::page_table::PhysAddr, vpn2_min: usize, vpn2_max: usize) {
    use crate::mm::frame;
    use crate::mm::page_table::PTEntry;

    let root_ptr = root.0 as *mut PTEntry;
    for vpn2 in vpn2_min..vpn2_max {
        let entry = unsafe { &mut *root_ptr.add(vpn2) };
        if !entry.is_valid() {
            continue;
        }
        let l2_addr = entry.ppn_to_addr();
        let l2_ptr = l2_addr.0 as *mut PTEntry;
        for vpn1 in 0..512 {
            let l2_entry = unsafe { &mut *l2_ptr.add(vpn1) };
            if !l2_entry.is_valid() {
                continue;
            }
            if !l2_entry.is_r() && !l2_entry.is_w() && !l2_entry.is_x() {
                let l1_addr = l2_entry.ppn_to_addr();
                frame::free_frame(l1_addr);
            }
        }
        frame::free_frame(l2_addr);
    }
}
