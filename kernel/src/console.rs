use core::fmt::Arguments;

#[allow(dead_code)]
pub fn print(args: Arguments) {
    crate::arch::stdout::stdout_puts(args);
}

#[allow(dead_code)]
pub fn error_print(args: Arguments) {
    crate::arch::stdout::stderr_puts(args);
}

/// 打印格式字串，无换行
#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?));
    }
}

/// 打印格式字串，有换行
#[macro_export]
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    }
}

/// 打印格式字串，使用与 println 不同的 Error 锁
#[macro_export]
macro_rules! error_println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::error_print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    }
}
