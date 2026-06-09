// syscall 分发表：从 trap_handler 中抽出，保持干净

use core::arch::asm;

use crate::{
    console,
    mm::{
        frame::{self, alloc_frame, dec_ref},
        page_table::{KERNEL_VPN2_MIN, PTEFlags, PTEntry, PageTable, VirtAddr},
    },
    task::{
        process::{Process, ProcessState, USER_HEAP_LIMIT},
        scheduler::{
            alloc_pid, current, find_empty_slot, reparent_orphans_to_init, schedule, wake_parent,
        },
    },
    trap,
};

const SYS_GETCWD: usize = 17;
const SYS_READ: usize = 63;
const SYS_WRITE: usize = 64;
const SYS_EXIT: usize = 93;
const SYS_EXIT_GROUP: usize = 94;
const SYS_UNAME: usize = 160;
const SYS_GETPID: usize = 172;
const SYS_GETPPID: usize = 173;
const SYS_GETUID: usize = 174;
const SYS_GETEUID: usize = 175;
const SYS_GETGID: usize = 176;
const SYS_GETEGID: usize = 177;
const SYS_GETTID: usize = 178;
const SYS_BRK: usize = 214;
const SYS_FORK: usize = 220;
const SYS_EXEC: usize = 221;
const SYS_WAIT: usize = 260;

const EAGAIN: isize = 11;
const EBADF: isize = 9;
const ECHILD: isize = 10;
const EFAULT: isize = 14;
const ERANGE: isize = 34;
const ENOSYS: isize = 38;

const SSTATUS_SUM: usize = 1 << 18;
const UTS_FIELD_LEN: usize = 65;
const UTS_FIELDS: usize = 6;
const UTS_SIZE: usize = UTS_FIELD_LEN * UTS_FIELDS;

pub fn dispatch(syscall_no: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    match syscall_no {
        SYS_GETCWD => sys_getcwd(arg0 as *mut u8, arg1),
        SYS_READ => sys_read(arg0, arg1 as *mut u8, arg2),
        SYS_WRITE => sys_write(arg0, arg1 as *const u8, arg2),
        SYS_EXIT | SYS_EXIT_GROUP => sys_exit(arg0),
        SYS_UNAME => sys_uname(arg0 as *mut u8),
        SYS_GETPID => current().pid,
        SYS_GETPPID => current().parent_pid,
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => 0,
        SYS_GETTID => current().pid,
        SYS_BRK => sys_brk(arg0),
        SYS_FORK => sys_fork(),
        SYS_EXEC => {
            // stub: 文件系统就绪后实现 ELF 加载
            let cur = current();
            cur.trap_frame.sepc = 0x10000;
            cur.trap_frame.set_sp(0x3F001000);
            cur.heap_end = cur.heap_start;
            0
        }
        SYS_WAIT => sys_wait(arg0 as *mut usize),
        _ => linux_error(ENOSYS),
    }
}

fn linux_error(errno: isize) -> usize {
    (-errno) as usize
}

fn with_user_access<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        asm!("csrs sstatus, {}", in(reg) SSTATUS_SUM);
    }
    let result = f();
    unsafe {
        asm!("csrc sstatus, {}", in(reg) SSTATUS_SUM);
    }
    result
}

fn ensure_user_range(ptr: usize, len: usize, write: bool) -> Result<(), usize> {
    if len == 0 {
        return Ok(());
    }
    if ptr == 0 {
        return Err(linux_error(EFAULT));
    }
    let end = ptr
        .checked_add(len - 1)
        .ok_or_else(|| linux_error(EFAULT))?;
    let mut page = ptr & !0xfff;
    loop {
        let entry = current()
            .page_table
            .lookup(VirtAddr(page))
            .ok_or_else(|| linux_error(EFAULT))?;
        if !entry.is_u() {
            return Err(linux_error(EFAULT));
        }
        if write {
            if entry.is_cow() && entry.is_r() && !entry.is_w() {
                trap::handle_cow_fault(VirtAddr(page), entry);
            }
            if !entry.is_w() {
                return Err(linux_error(EFAULT));
            }
        } else if !entry.is_r() {
            return Err(linux_error(EFAULT));
        }
        if page >= (end & !0xfff) {
            break;
        }
        page += 4096;
    }
    Ok(())
}

fn sys_read(fd: usize, buf: *mut u8, len: usize) -> usize {
    if fd != 0 {
        return linux_error(EBADF);
    }
    if let Err(errno) = ensure_user_range(buf as usize, len, true) {
        return errno;
    }
    with_user_access(|| {
        for i in 0..len {
            unsafe {
                *buf.add(i) = console::read_char();
            }
        }
    });
    len
}

fn sys_write(fd: usize, buf: *const u8, len: usize) -> usize {
    if fd != 1 && fd != 2 {
        return linux_error(EBADF);
    }
    if let Err(errno) = ensure_user_range(buf as usize, len, false) {
        return errno;
    }
    with_user_access(|| {
        for i in 0..len {
            console::putchar(unsafe { *buf.add(i) });
        }
    });
    len
}

fn sys_exit(code: usize) -> ! {
    reparent_orphans_to_init();

    current()
        .page_table
        .for_each_leaf(0, KERNEL_VPN2_MIN, &mut |_vaddr, entry: &mut PTEntry| {
            if !entry.flags().is_u() {
                return;
            }
            let pa = entry.ppn_to_addr();
            dec_ref(pa);
            *entry = PTEntry::empty();
        });
    unsafe {
        asm!("sfence.vma");
    }

    current().state = crate::task::process::ProcessState::Zombie(code);
    wake_parent();
    schedule()
}

fn sys_getcwd(buf: *mut u8, size: usize) -> usize {
    // stage-1: no VFS/cwd state yet. The only supported cwd is the root.
    const CWD: &[u8] = b"/\0";
    if size < CWD.len() {
        return linux_error(ERANGE);
    }
    if let Err(errno) = ensure_user_range(buf as usize, CWD.len(), true) {
        return errno;
    }
    with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(CWD.as_ptr(), buf, CWD.len());
    });
    CWD.len()
}

fn sys_uname(buf: *mut u8) -> usize {
    if let Err(errno) = ensure_user_range(buf as usize, UTS_SIZE, true) {
        return errno;
    }
    with_user_access(|| unsafe {
        core::ptr::write_bytes(buf, 0, UTS_SIZE);
        write_uts_field(buf, 0, b"Unit00");
        write_uts_field(buf, 1, b"unit00");
        write_uts_field(buf, 2, b"0.1.0");
        write_uts_field(buf, 3, b"oskernel-2026 stage-1");
        write_uts_field(buf, 4, b"riscv64");
        write_uts_field(buf, 5, b"localdomain");
    });
    0
}

unsafe fn write_uts_field(buf: *mut u8, index: usize, value: &[u8]) {
    let len = core::cmp::min(value.len(), UTS_FIELD_LEN - 1);
    unsafe {
        core::ptr::copy_nonoverlapping(value.as_ptr(), buf.add(index * UTS_FIELD_LEN), len);
    }
}

fn sys_brk(new_break: usize) -> usize {
    let old_break = current().heap_end;
    let heap_start = current().heap_start;

    if new_break == 0 {
        return old_break;
    }
    if new_break < heap_start || new_break > USER_HEAP_LIMIT {
        return old_break;
    }

    if new_break > old_break {
        if !grow_heap(old_break, new_break) {
            return old_break;
        }
    } else if new_break < old_break {
        shrink_heap(new_break, old_break);
    }

    current().heap_end = new_break;
    new_break
}

fn grow_heap(old_break: usize, new_break: usize) -> bool {
    let start = align_up(old_break);
    let end = align_up(new_break);
    let mut vaddr = start;

    while vaddr < end {
        let frame = match alloc_frame() {
            Some(frame) => frame,
            None => {
                unmap_heap_pages(start, vaddr);
                return false;
            }
        };
        unsafe {
            core::ptr::write_bytes(frame.0 as *mut u8, 0, 4096);
        }

        current().page_table.map(
            VirtAddr(vaddr),
            frame,
            PTEFlags::new(true, true, false, true),
        );
        if current().page_table.lookup(VirtAddr(vaddr)).is_none() {
            frame::free_frame(frame);
            unmap_heap_pages(start, vaddr);
            return false;
        }
        vaddr += 4096;
    }

    unsafe {
        asm!("sfence.vma");
    }
    true
}

fn shrink_heap(new_break: usize, old_break: usize) {
    unmap_heap_pages(align_up(new_break), align_up(old_break));
}

fn unmap_heap_pages(start: usize, end: usize) {
    let mut vaddr = start;
    while vaddr < end {
        if let Some(entry) = current().page_table.lookup(VirtAddr(vaddr))
            && entry.is_valid()
            && entry.is_u()
        {
            let pa = entry.ppn_to_addr();
            dec_ref(pa);
            *entry = PTEntry::empty();
        }
        vaddr += 4096;
    }
    unsafe {
        asm!("sfence.vma");
    }
}

fn align_up(addr: usize) -> usize {
    (addr + 4095) & !4095
}

fn sys_fork() -> usize {
    let idx = match find_empty_slot() {
        Some(i) => i,
        None => return linux_error(EAGAIN),
    };
    let pid = alloc_pid();

    let root_frame = alloc_frame().expect("fork: no frame for root page table");
    unsafe {
        core::ptr::write_bytes(root_frame.0 as *mut u8, 0, 4096);
    }

    let parent_root = current().page_table.root_addr().0 as *const u64;
    let child_root = root_frame.0 as *mut u64;
    for vpn2 in KERNEL_VPN2_MIN..512 {
        let pte = unsafe { core::ptr::read(parent_root.add(vpn2)) };
        if pte != 0 {
            unsafe {
                core::ptr::write(child_root.add(vpn2), pte);
            }
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

    current()
        .page_table
        .for_each_leaf(0, KERNEL_VPN2_MIN, &mut |vaddr, entry: &mut PTEntry| {
            if !entry.is_u() {
                return;
            }
            let pa = entry.ppn_to_addr();
            frame::inc_ref(pa);
            if entry.is_w() {
                entry.clear_w();
                entry.set_cow();
            }
            if entry.is_cow() {
                child_pt.map_cow(vaddr, pa, entry.flags());
            } else {
                child_pt.map(vaddr, pa, entry.flags());
            }
        });

    current()
        .page_table
        .for_each_leaf(0, KERNEL_VPN2_MIN, &mut |vaddr, entry: &mut PTEntry| {
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
        heap_start: current().heap_start,
        heap_end: current().heap_end,
    };

    unsafe {
        crate::task::scheduler::PROCESS_LIST[idx] = Some(child);
    }

    unsafe {
        asm!("sfence.vma");
    }

    parent_ret
}

fn sys_wait(exit_code_ptr: *mut usize) -> usize {
    use crate::task::scheduler::{MAX_PROCESSES, PROCESS_LIST};

    let cur_pid = current().pid;
    let mut zombie: Option<(usize, usize)> = None;

    let process_ptr = unsafe { PROCESS_LIST.as_ptr() };
    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &*process_ptr.add(i) };
        if let Some(child) = slot
            && child.parent_pid == cur_pid
            && let ProcessState::Zombie(code) = child.state
        {
            zombie = Some((i, code));
            break;
        }
    }

    if let Some((idx, code)) = zombie {
        if !exit_code_ptr.is_null() {
            // 主动处理 COW：防止内核写用户 wait 指针时触发嵌套 page fault。
            let vaddr = VirtAddr(exit_code_ptr as usize);
            if let Some(entry) = current().page_table.lookup(vaddr)
                && entry.is_cow()
                && entry.is_r()
                && !entry.is_w()
                && entry.is_u()
            {
                trap::handle_cow_fault(vaddr, entry);
            }
            unsafe {
                asm!("csrs sstatus, {}", in(reg) 1usize << 18);
                core::ptr::write(exit_code_ptr, code);
                asm!("csrc sstatus, {}", in(reg) 1usize << 18);
            }
        }

        let process_ptr = unsafe { PROCESS_LIST.as_mut_ptr() };
        let slot = unsafe { &mut *process_ptr.add(idx) };
        let child = slot.as_mut().expect("wait: zombie slot disappeared");
        let child_pid = child.pid;
        free_user_pt_resources(child);
        child.state = ProcessState::Gone;
        return child_pid;
    }

    let process_ptr = unsafe { PROCESS_LIST.as_ptr() };
    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &*process_ptr.add(i) };
        if let Some(child) = slot
            && child.parent_pid == cur_pid
            && child.state != ProcessState::Gone
            && !matches!(child.state, ProcessState::Zombie(_))
        {
            current().state = ProcessState::Blocked;
            schedule();
        }
    }

    linux_error(ECHILD)
}

fn free_user_pt_resources(child: &mut Process) {
    use crate::mm::frame;

    frame::free_frame(child.kernel_stack_frame);

    let root_addr = child.page_table.root_addr();
    free_intermediate_frames(root_addr, 0, KERNEL_VPN2_MIN);

    frame::free_frame(root_addr);
}

fn free_intermediate_frames(
    root: crate::mm::page_table::PhysAddr,
    vpn2_min: usize,
    vpn2_max: usize,
) {
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
