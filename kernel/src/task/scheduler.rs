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
    let sp: usize;
    unsafe { asm!("mv {}, sp", out(reg) sp) };

    // 内核栈布局：[ra:0] [a0:8] [a1:16] [a2:24] [a7:32] [scause:40] [sepc:48] [stval:56]
    unsafe {
        cur.trap_frame.ra = core::ptr::read(sp as *const usize);
        cur.trap_frame.a0 = core::ptr::read((sp + 8) as *const usize);
        cur.trap_frame.a1 = core::ptr::read((sp + 16) as *const usize);
        cur.trap_frame.a2 = core::ptr::read((sp + 24) as *const usize);
        cur.trap_frame.a7 = core::ptr::read((sp + 32) as *const usize);
        cur.trap_frame.scause = core::ptr::read((sp + 40) as *const usize);
        cur.trap_frame.sepc = core::ptr::read((sp + 48) as *const usize);
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

    // 切页表
    let satp = next.page_table.satp_val();
    unsafe {
        asm!("csrw satp, {}", in(reg) satp);
        asm!("sfence.vma");
    }

    // 4. 恢复新进程上下文
    // sscratch = 用户 sp（trap_exit 的 csrrw 会换回）
    unsafe { asm!("csrw sscratch, {}", in(reg) next.trap_frame.sp) };
    // sp = kernel_sp - 64（模拟 trap_entry 刚 push 完的状态）
    let new_sp = next.kernel_sp - 64;
    unsafe { asm!("mv sp, {}", in(reg) new_sp) };
    // 将 TrapFrame 字段写回内核栈
    unsafe {
        core::ptr::write(new_sp as *mut usize, next.trap_frame.ra);
        core::ptr::write((new_sp + 8) as *mut usize, next.trap_frame.a0);
        core::ptr::write((new_sp + 16) as *mut usize, next.trap_frame.a1);
        core::ptr::write((new_sp + 24) as *mut usize, next.trap_frame.a2);
        core::ptr::write((new_sp + 32) as *mut usize, next.trap_frame.a7);
        core::ptr::write((new_sp + 40) as *mut usize, next.trap_frame.scause);
        core::ptr::write((new_sp + 48) as *mut usize, next.trap_frame.sepc);
    }
    // 写 sstatus
    unsafe { asm!("csrw sstatus, {}", in(reg) next.trap_frame.sstatus) };

    // 5. 跳转到 trap_exit_restore（外部 asm 标签，Task 5 中定义）
    unsafe {
        asm!("j {}", in(reg) crate::trap::TRAP_EXIT_RESTORE_ADDR, options(noreturn));
    }
}
