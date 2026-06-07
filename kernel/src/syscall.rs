// syscall 分发表：从 trap_handler 中抽出，保持干净

use core::arch::asm;

use crate::{
    console,
    mm::{
        frame::{self, alloc_frame, dec_ref},
        page_table::{KERNEL_VPN2_MIN, PTEntry, PageTable},
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
            // getpid() 暂时实现
            1
        }
        220 => sys_fork(),
        221 => {
            // exec: stub（文件系统就绪后实现 ELF 加载）
            let cur = current();
            cur.trap_frame.sepc = 0x10000;    // 硬编码用户代码入口
            cur.trap_frame.sp = 0x3F001000;   // 硬编码用户栈顶
            0
        }
        260 => {
            // wait(exit_code_ptr)
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
    // 步骤 1: 找空 slot + 分配 pid
    let idx = match find_empty_slot() {
        Some(i) => i,
        None => return (-11isize) as usize, // EAGAIN
    };
    let pid = alloc_pid();

    // 步骤 2: 分配子进程根页表帧 + 清零
    let root_frame = alloc_frame().expect("fork: no frame for root page table");
    unsafe { core::ptr::write_bytes(root_frame.0 as *mut u8, 0, 4096); }

    // 步骤 3: 复制 VPN[2] >= KERNEL_VPN2_MIN 的根 PTE（共享内核中间页表）
    let parent_root = current().page_table.root_addr().0 as *const u64;
    let child_root = root_frame.0 as *mut u64;
    for vpn2 in KERNEL_VPN2_MIN..512 {
        let pte = unsafe { core::ptr::read(parent_root.add(vpn2)) };
        if pte != 0 {
            unsafe { core::ptr::write(child_root.add(vpn2), pte); }
        }
    }

    // 步骤 4: 分配子进程内核栈帧
    let kernel_stack = alloc_frame().expect("fork: no frame for kernel stack");
    let kernel_sp = kernel_stack.0 + 4096;

    // 步骤 5: 拷贝父进程 TrapFrame + 子进程修改
    let mut child_tf = current().trap_frame;
    child_tf.a0 = 0;       // 子进程 fork 返回 0
    child_tf.sepc += 4;    // 跳过 ecall，子进程首次运行从下一条指令开始
    child_tf.scause = 8;   // 仅作记录，trap_exit_restore 不检查

    // 步骤 6: 父进程返回值 = 子 pid
    let parent_ret = pid;

    // 步骤 7: 遍历父进程用户叶子（U=1）→ COW 共享 + inc_ref
    current().page_table.for_each_leaf(0, KERNEL_VPN2_MIN, &mut |_vaddr, entry: &mut PTEntry| {
        if !entry.is_u() {
            return;
        }
        let pa = entry.ppn_to_addr();
        frame::inc_ref(pa);
        if entry.is_w() {
            entry.clear_w();   // W=0
            entry.set_cow();   // COW=1
        }
        // 原本只读（代码段 R+X）→ COW=0 不变，但已 inc_ref 保护生命周期
    });

    // 步骤 8: 创建子进程页表 + 重映射 UART MMIO（U=0, VPN[2] < KERNEL_VPN2_MIN）
    let mut child_pt = PageTable::new(root_frame, alloc_frame);
    current().page_table.for_each_leaf(0, KERNEL_VPN2_MIN, &mut |vaddr, entry: &mut PTEntry| {
        if entry.is_u() {
            return;
        }
        child_pt.map(vaddr, entry.ppn_to_addr(), entry.flags());
    });

    // 步骤 9: 构建子进程 + 入表 + sfence.vma
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

    // 刷新父进程 TLB（父的 PTE 被修改了：W→0, COW→1）
    unsafe { asm!("sfence.vma"); }

    parent_ret
}

fn sys_wait(exit_code_ptr: *mut usize) -> usize {
    use crate::task::scheduler::{PROCESS_LIST, MAX_PROCESSES};

    let cur_pid = current().pid;

    // 1. 找 Zombie 子进程
    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &mut PROCESS_LIST[i] };
        if let Some(child) = slot {
            if child.parent_pid == cur_pid {
                if let ProcessState::Zombie(code) = child.state {
                    // 2. 找到 → 写退出码到用户内存
                    if !exit_code_ptr.is_null() {
                        unsafe {
                            asm!("csrs sstatus, {}", in(reg) 1usize << 18); // 开 SUM
                            core::ptr::write(exit_code_ptr, code);
                            asm!("csrc sstatus, {}", in(reg) 1usize << 18); // 关 SUM
                        }
                    }

                    // 释放子进程私有资源
                    free_user_pt_resources(child);

                    child.state = ProcessState::Gone;
                    return child.pid;
                }
            }
        }
    }

    // 3. 有活子进程 → Blocked
    for i in 0..MAX_PROCESSES {
        let slot = unsafe { &PROCESS_LIST[i] };
        if let Some(child) = slot {
            if child.parent_pid == cur_pid
                && child.state != ProcessState::Gone
                && !matches!(child.state, ProcessState::Zombie(_))
            {
                // 有活子进程，当前进程阻塞
                current().state = ProcessState::Blocked;
                schedule();
                // schedule() 不返回，被唤醒后 trap_handler 重新调用 wait
            }
        }
    }

    // 4. 无子进程
    (-10isize) as usize // ECHILD = 10
}

fn free_user_pt_resources(child: &mut Process) {
    use crate::mm::frame;

    // 释放内核栈帧
    frame::free_frame(child.kernel_stack_frame);

    // 释放 VPN[2] < KERNEL_VPN2_MIN 的中间页表帧 + 根页表帧
    let root_addr = child.page_table.root_addr();
    free_intermediate_frames(root_addr, 0, KERNEL_VPN2_MIN);

    // 释放根页表帧本身
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
            // 非叶子 → L1 帧
            if !l2_entry.is_r() && !l2_entry.is_w() && !l2_entry.is_x() {
                let l1_addr = l2_entry.ppn_to_addr();
                // L1 帧内的叶子已在 exit / free_user_pt 中 dec_ref
                frame::free_frame(l1_addr);
            }
        }
        // 释放 L2 帧
        frame::free_frame(l2_addr);
    }
}
