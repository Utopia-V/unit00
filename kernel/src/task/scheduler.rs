use crate::task::process::{Process, ProcessState};
use core::arch::asm;

pub(crate) const MAX_PROCESSES: usize = 32;

pub(crate) static mut PROCESS_LIST: [Option<Process>; MAX_PROCESSES] = [const { None }; MAX_PROCESSES];

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
            if let Some(ref mut proc) = *slot {
                if proc.parent_pid == cur_pid && proc.state != ProcessState::Gone {
                    proc.parent_pid = 1;
                }
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
            if let Some(ref mut proc) = *slot {
                if proc.pid == parent_pid && proc.state == ProcessState::Blocked {
                    proc.state = ProcessState::Ready;
                    break;
                }
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
            if let Some(ref proc) = *ptr.add(idx) {
                if proc.state == ProcessState::Ready {
                    return idx;
                }
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
    // trap frame 固定在 kernel_sp - 64（由 trap_entry 写入），
    // 不能用当前 sp，因为 schedule() 调用链中 sp 早已离开该位置。
    let frame_base = cur.kernel_sp - 64;
    unsafe {
        cur.trap_frame.ra = core::ptr::read(frame_base as *const usize);
        cur.trap_frame.a0 = core::ptr::read((frame_base + 8) as *const usize);
        cur.trap_frame.a1 = core::ptr::read((frame_base + 16) as *const usize);
        cur.trap_frame.a2 = core::ptr::read((frame_base + 24) as *const usize);
        cur.trap_frame.a7 = core::ptr::read((frame_base + 32) as *const usize);
        cur.trap_frame.scause = core::ptr::read((frame_base + 40) as *const usize);
        cur.trap_frame.sepc = core::ptr::read((frame_base + 48) as *const usize);
    }
    // 从 sscratch 读用户 sp（trap 期间 sscratch 持有用户 sp）
    unsafe { asm!("csrr {}, sscratch", out(reg) cur.trap_frame.sp) };
    // 从 sstatus 读 SPP 位
    unsafe { asm!("csrr {}, sstatus", out(reg) cur.trap_frame.sstatus) };

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
    let new_sp = next.kernel_sp - 64;
    let user_sp = next.trap_frame.sp;
    let sstatus = next.trap_frame.sstatus;
    let ra   = next.trap_frame.ra;
    let a0   = next.trap_frame.a0;
    let a1   = next.trap_frame.a1;
    let a2   = next.trap_frame.a2;
    let a7   = next.trap_frame.a7;
    let scause = next.trap_frame.scause;
    let sepc = next.trap_frame.sepc;
    let restore_addr = unsafe { crate::trap::TRAP_EXIT_RESTORE_ADDR };

    // 切页表、写 trap frame、切换 sscratch/sp/sstatus、跳转 ——
    // 全部放在一个 asm 块里。mv sp 之后编译器不能再插入任何指令。
    unsafe {
        asm!(
            "csrw satp, {next_satp}",
            "sfence.vma",
            "sd   {ra},  0({new_sp})",
            "sd   {a0},  8({new_sp})",
            "sd   {a1}, 16({new_sp})",
            "sd   {a2}, 24({new_sp})",
            "sd   {a7}, 32({new_sp})",
            "sd   {scause}, 40({new_sp})",
            "sd   {sepc}, 48({new_sp})",
            "csrw sscratch, {user_sp}",
            "mv   sp, {new_sp}",
            "csrw sstatus, {sstatus}",
            "jr   {restore_addr}",
            next_satp = in(reg) next_satp,
            new_sp = in(reg) new_sp,
            user_sp = in(reg) user_sp,
            ra = in(reg) ra,
            a0 = in(reg) a0,
            a1 = in(reg) a1,
            a2 = in(reg) a2,
            a7 = in(reg) a7,
            scause = in(reg) scause,
            sepc = in(reg) sepc,
            sstatus = in(reg) sstatus,
            restore_addr = in(reg) restore_addr,
            options(noreturn),
        );
    }
}
