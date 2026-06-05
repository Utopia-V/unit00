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
