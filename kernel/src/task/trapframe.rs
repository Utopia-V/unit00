use core::ptr;

const REG_SP: usize = 2;
const REG_A0: usize = 10;
const SSTATUS_SPIE: usize = 1 << 5;

/// Full user register snapshot saved by `trap_entry`.
///
/// Stack layout is exactly:
/// - `regs[0..32]` at offsets 0..256
/// - `sstatus`, `sepc`, `scause`, `stval` at offsets 256..288
///
/// `regs[0]` is kept as zero for indexing clarity. `regs[2]` is the user sp.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    pub scause: usize,
    pub stval: usize,
}

impl TrapFrame {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    pub const fn new_user(entry: usize, user_sp: usize) -> Self {
        let mut regs = [0; 32];
        regs[REG_SP] = user_sp;
        Self {
            regs,
            // SPP=0 returns to U-mode. SPIE=1 makes S-mode interrupts enabled
            // after sret, without enabling them while we are still in kernel.
            sstatus: SSTATUS_SPIE,
            sepc: entry,
            scause: 8,
            stval: 0,
        }
    }

    pub fn set_a0(&mut self, value: usize) {
        self.regs[REG_A0] = value;
    }

    pub fn set_sp(&mut self, value: usize) {
        self.regs[REG_SP] = value;
    }

    /// Read the trap frame written by assembly at `frame_base`.
    ///
    /// # Safety
    ///
    /// `frame_base` must point to a complete `TrapFrame` stack image written by
    /// `trap_entry` on a live kernel stack.
    pub unsafe fn read_from_stack(frame_base: usize) -> Self {
        unsafe { ptr::read(frame_base as *const Self) }
    }

    /// Write this trap frame to `frame_base` before jumping to `trap_exit_restore`.
    ///
    /// # Safety
    ///
    /// `frame_base` must point to writable kernel-stack space of `TrapFrame::SIZE`
    /// bytes and must be 16-byte aligned for the restore path.
    pub unsafe fn write_to_stack(&self, frame_base: usize) {
        unsafe { ptr::write(frame_base as *mut Self, *self) };
    }
}

const _: () = {
    assert!(core::mem::offset_of!(TrapFrame, regs) == 0);
    assert!(core::mem::offset_of!(TrapFrame, sstatus) == 256);
    assert!(core::mem::offset_of!(TrapFrame, sepc) == 264);
    assert!(core::mem::offset_of!(TrapFrame, scause) == 272);
    assert!(core::mem::offset_of!(TrapFrame, stval) == 280);
    assert!(TrapFrame::SIZE == 288);
};
