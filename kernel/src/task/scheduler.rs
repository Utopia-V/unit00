use crate::task::process::{Process, ProcessState};
use crate::task::trapframe::TrapFrame;
use core::arch::asm;

pub(crate) const MAX_PROCESSES: usize = 32;

pub(crate) static mut PROCESS_LIST: [Option<Process>; MAX_PROCESSES] =
    [const { None }; MAX_PROCESSES];

pub(crate) static mut CURRENT: usize = 0;
static mut NEXT_PID: usize = 1;

// ── 简单 helper ──

pub fn find_empty_slot() -> Option<usize> {
    for (i, slot) in unsafe { PROCESS_LIST.iter().enumerate() } {
        if slot.is_none() || matches!(slot, Some(p) if p.state == ProcessState::Gone) {
            return Some(i);
        }
    }
    None
}

pub fn alloc_pid() -> usize {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}

pub fn current() -> &'static mut Process {
    unsafe {
        PROCESS_LIST[CURRENT]
            .as_mut()
            .expect("current(): PROCESS_LIST[CURRENT] is None")
    }
}

#[allow(unused)]
pub fn current_index() -> usize {
    unsafe { CURRENT }
}

// ── 孤儿重亲：当前进程的所有子进程 → init (pid=1) ──

pub fn reparent_orphans_to_init() {
    let cur_pid = current().pid;
    let ptr = unsafe { PROCESS_LIST.as_mut_ptr() };
    for i in 0..MAX_PROCESSES {
        unsafe {
            let slot = &mut *ptr.add(i);
            if let Some(proc) = slot.as_mut()
                && proc.parent_pid == cur_pid
                && proc.state != ProcessState::Gone
            {
                proc.parent_pid = 1;
            }
        }
    }
}

// ── 唤醒等待当前进程的父进程 ──

pub fn wake_parent() {
    let parent_pid = current().parent_pid;
    let ptr = unsafe { PROCESS_LIST.as_mut_ptr() };
    for i in 0..MAX_PROCESSES {
        unsafe {
            let slot = &mut *ptr.add(i);
            if let Some(proc) = slot.as_mut()
                && proc.pid == parent_pid
                && proc.state == ProcessState::Blocked
            {
                proc.state = ProcessState::Ready;
                break;
            }
        }
    }
}

// ── 选下一个 Ready 进程 ──

fn pick_next() -> usize {
    let cur = unsafe { CURRENT };
    let ptr = unsafe { PROCESS_LIST.as_ptr() };
    for i in 1..=MAX_PROCESSES {
        let idx = (cur + i) % MAX_PROCESSES;
        unsafe {
            let slot = &*ptr.add(idx);
            if let Some(proc) = slot.as_ref()
                && proc.state == ProcessState::Ready
            {
                return idx;
            }
        }
    }
    MAX_PROCESSES // sentinel: 无可运行进程
}

// ── 调度核心 ──

/// 保存当前进程 → 选新进程 → 恢复新进程 → 跳转 trap_exit_restore
/// 不返回。
pub fn schedule() -> ! {
    // 1. 保存当前进程上下文
    let cur = current();
    // trap frame 固定在 kernel_sp - TrapFrame::SIZE（由 trap_entry 写入），
    // 不能用当前 sp，因为 schedule() 调用链中 sp 早已离开该位置。
    let frame_base = cur.kernel_sp - TrapFrame::SIZE;
    unsafe {
        cur.trap_frame = TrapFrame::read_from_stack(frame_base);
    }

    // 2. 选新进程
    let next_idx = pick_next();
    if next_idx == MAX_PROCESSES {
        crate::sbi::shutdown();
    }

    // 3. 切换硬件状态
    unsafe { CURRENT = next_idx };
    let next = current();
    next.state = ProcessState::Running;

    // 提前读出所有需要的值（mv sp 之后不能再访问 Rust 局部变量）
    let next_satp = next.page_table.satp_val();
    let new_sp = next.kernel_sp - TrapFrame::SIZE;
    let restore_addr = unsafe { crate::trap::TRAP_EXIT_RESTORE_ADDR };
    unsafe {
        next.trap_frame.write_to_stack(new_sp);
    }

    // 切页表、写 trap frame、切换 sscratch/sp/sstatus、跳转 ——
    // 全部放在一个 asm 块里。mv sp 之后编译器不能再插入任何指令。
    unsafe {
        asm!(
            "csrw satp, {next_satp}",
            "sfence.vma",
            "mv   sp, {new_sp}",
            "jr   {restore_addr}",
            next_satp = in(reg) next_satp,
            new_sp = in(reg) new_sp,
            restore_addr = in(reg) restore_addr,
            options(noreturn),
        );
    }
}
