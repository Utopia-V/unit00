const UART0: usize = 0x1000_0000;

struct Uart16550 {
    base: usize,
}

impl Uart16550 {
    fn putchar(&self, c: u8) {
        while self.read_reg(5) & (1 << 5) == 0 {
            // spin until THR empty
        }
        self.write_reg(0, c);
    }

    fn putstr(&self, s: &str) {
        for c in s.bytes() {
            if c == b'\n' {
                self.putchar(b'\r');
            }
            self.putchar(c);
        }
    }

    fn read_char(&self) -> u8 {
        while self.read_reg(5) & (1 << 0) == 0 {}
        self.read_reg(0)
    }

    fn read_reg(&self, offset: usize) -> u8 {
        unsafe { ((self.base + offset) as *const u8).read_volatile() }
    }

    fn write_reg(&self, offset: usize, val: u8) {
        unsafe { ((self.base + offset) as *mut u8).write_volatile(val) }
    }
}

static UART: Uart16550 = Uart16550 { base: UART0 };

pub fn puts(s: &str) {
    UART.putstr(s);
}

pub fn putchar(c: u8) {
    UART.putchar(c);
}

pub fn read_char() -> u8 {
    UART.read_char()
}
