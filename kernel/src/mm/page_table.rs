// SV39 页表实现：walk / map / unmap + 内核恒等映射初始化

use core::ptr;

// ─── 地址类型 ───

#[derive(Clone, Copy, PartialEq)]
pub struct PhysAddr(pub usize);

#[derive(Clone, Copy)]
pub struct VirtAddr(pub usize);

// ─── 页表项 ───

#[derive(Clone, Copy)]
pub struct PTEntry(u64);

/// 页表项的标志位
#[derive(Clone, Copy)]
pub struct PTEFlags(u8);

// bit 在 u8 中的位置
const V_BIT: u8 = 0;
const R_BIT: u8 = 1;
const W_BIT: u8 = 2;
const X_BIT: u8 = 3;
const U_BIT: u8 = 4;

impl PTEntry {
    /// 空项（V=0，无效）
    fn empty() -> Self { Self(0) }

    /// 指向下一级页表的条目
    fn new_table(paddr: PhysAddr) -> Self {
        let ppn = paddr.0 >> 12;           // 物理地址去掉低 12 位
        Self((ppn << 10) as u64 | (1 << V_BIT as u64))
    }

    /// 指向最终物理页的条目（带读写等标志）
    fn new_leaf(paddr: PhysAddr, flags: PTEFlags) -> Self {
        let ppn = paddr.0 >> 12;
        Self((ppn << 10) as u64 | flags.into_u64())
    }

    fn is_valid(&self) -> bool { self.0 & 1 != 0 }

    /// 取物理页号 → 物理地址
    fn ppn_to_addr(&self) -> PhysAddr {
        PhysAddr((((self.0 >> 10) & 0xFFFF_FFFF_FFFF) as usize) << 12)
    }
}

impl PTEFlags {
    pub fn new(r: bool, w: bool, x: bool, u: bool) -> Self {
        let mut f = 1u8 << V_BIT; // V 总是 1
        if r { f |= 1 << R_BIT; }
        if w { f |= 1 << W_BIT; }
        if x { f |= 1 << X_BIT; }
        if u { f |= 1 << U_BIT; }
        Self(f)
    }

    fn into_u64(self) -> u64 { self.0 as u64 }
}

// ─── 页表 ───

/// SV39 页表。root 是根页表的物理地址。
pub struct PageTable {
    root: PhysAddr,       // 第 3 层页表物理地址
    frame_alloc: fn() -> Option<PhysAddr>,  // 临时：分配一页物理内存
}

impl PageTable {
    /// 创建一个空页表。root 必须是已分配好的一页物理内存。
    pub fn new(root: PhysAddr, frame_alloc: fn() -> Option<PhysAddr>) -> Self {
        Self { root, frame_alloc }
    }

    /// 查找虚拟地址对应的页表项（可变引用）
    /// 如果中间层页表不存在，自动分配
    fn walk(&mut self, vaddr: VirtAddr) -> Option<&mut PTEntry> {
        let vpn = [ // 取三层 VPN
            (vaddr.0 >> 12) & 0x1FF,  // 第 1 层（VPN0）
            (vaddr.0 >> 21) & 0x1FF,  // 第 2 层（VPN1）
            (vaddr.0 >> 30) & 0x1FF,  // 第 3 层（VPN2）
        ];

        // 从根页表开始
        let mut table_addr = self.root;

        // 走第 3 层和第 2 层（中间层）
        for level in (1..=2).rev() {
            let idx = vpn[level];
            let entry = Self::entry_at(table_addr, idx);

            if !entry.is_valid() {
                // 中间层不存在 → 分配新页表
                let new_page = (self.frame_alloc)()?;
                // 清空新页表
                unsafe { ptr::write_bytes(new_page.0 as *mut u8, 0, 4096); }
                // 在父页表里填上指向新页表的条目
                *entry = PTEntry::new_table(new_page);
            }

            table_addr = entry.ppn_to_addr();
        }

        // 第 1 层：返回叶子项
        Some(Self::entry_at(table_addr, vpn[0]))
    }

    /// 把一个虚拟页映射到一个物理页
    pub fn map(&mut self, vaddr: VirtAddr, paddr: PhysAddr, flags: PTEFlags) {
        if let Some(entry) = self.walk(vaddr) {
            *entry = PTEntry::new_leaf(paddr, flags);
        }
    }

    /// 解除映射
    pub fn unmap(&mut self, vaddr: VirtAddr) {
        if let Some(entry) = self.walk(vaddr) {
            *entry = PTEntry::empty();
        }
    }

    /// 返回 satp 寄存器的值（准备启用该页表时的写值）
    pub fn satp_val(&self) -> usize {
        // satp[63:60] = MODE (8 = SV39)
        // satp[43:0]  = 根页表 PPN
        (8usize << 60) | (self.root.0 >> 12)
    }

    // ─── 内部辅助 ───

    /// 取页表第 idx 项的指针
    fn entry_at(table_addr: PhysAddr, idx: usize) -> &'static mut PTEntry {
        let ptr = table_addr.0 as *mut PTEntry;
        unsafe { &mut *ptr.add(idx) }
    }
}

// ─── 内核初始恒等映射 ───

/// 为内核初始化页表：把物理地址 [start, end) 恒等映射
/// 返回页表（调用方可继续 map 其他区域）+ satp 值
pub fn init_kernel(start: PhysAddr, end: PhysAddr, frame_alloc: fn() -> Option<PhysAddr>) -> (PageTable, usize) {
    let root = (frame_alloc)().expect("no frame for root page table");
    unsafe { ptr::write_bytes(root.0 as *mut u8, 0, 4096); }

    let mut pt = PageTable::new(root, frame_alloc);
    let flags = PTEFlags::new(true, true, true, false);  // R+W+X, 仅内核

    let mut pa = start;
    while pa.0 < end.0 {
        pt.map(VirtAddr(pa.0), pa, flags);
        pa.0 += 4096;
    }

    let satp = pt.satp_val();
    (pt, satp)
}
