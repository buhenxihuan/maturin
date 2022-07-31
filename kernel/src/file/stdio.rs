//! 标准输入输出流的 File 封装
//! 
//! 输出流调用的 print! 和 error_print! 宏是由 crate::arch::stdout 提供的。
//! 保证多核调用时不互相冲突的 Mutex 锁也在 crate::arch::{stdin, stdout} 中实现，这个模块只是封装了 Trait File

//#![deny(missing_docs)]

use super::File;
use crate::arch::stdin::getchar;

/// 标准输入流
pub struct Stdin;
/// 标准输出流
pub struct Stdout;
/// 错误输出流。目前会和 Stdout 一样直接打印出来，但用的锁和 Stdout 不同
pub struct Stderr;

impl File for Stdin {
    /// 目前 Stdin 只支持读一个字符
    fn read(&self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() == 0 {
            return Some(0)
        }
        buf[0] = loop {
            // 目前调用 sys_read 会导致当前进程阻塞在用户输入上
            let c = getchar();
            if c == 0 || c == 255 {
                continue;
            } else {
                break c;
            }
        };
        Some(1)
    }
    /// Stdin 不可写
    fn write(&self, buf: &[u8]) -> Option<usize> {
        None
    }
}

impl File for Stdout {
    /// Stdout 不可读
    fn read(&self, buf: &mut [u8]) -> Option<usize> {
        None
    }
    /// UTF-8 格式写
    fn write(&self, buf: &[u8]) -> Option<usize> {
        if let Ok(data) = core::str::from_utf8(buf) {
            print!("{}", data);
            Some(buf.len())
        } else {
            None
        }
    }
}

impl File for Stderr {
    /// Stdout 不可读
    fn read(&self, buf: &mut [u8]) -> Option<usize> {
        None
    }
    /// UTF-8 格式写
    fn write(&self, buf: &[u8]) -> Option<usize> {
        if let Ok(data) = core::str::from_utf8(buf) {
            error_print!("{}", data);
            Some(buf.len())
        } else {
            for i in 0..buf.len() {
                error_print!("{} ", buf[i]);
            }
            Some(buf.len())
        }
    }
}