//! 打开文件时的权限和选项

#![deny(missing_docs)]

use bitflags::*;

bitflags! {    
    /// 指定文件打开时的权限
    pub struct OpenFlags: u32 {
        /// 只读
        const RDONLY = 0;
        /// 只能写入
        const WRONLY = 1 << 0;
        /// 读写
        const RDWR = 1 << 1;
        /// 如文件不存在，可创建它
        const CREATE = 1 << 6;
        /// 确认一定是创建文件。如文件已存在，返回 EEXIST。
        const EXCL = 1 << 9;
        /// 没查到，但 date.lua 要
        const UNKNOWN = 1 << 11;
        /// 要求把 CR-LF 都换成 LF
        const TEXT = 1 << 14;
        /// 和上面不同，要求输入输出都不进行这个翻译
        const BINARY = 1 << 15;
        /// 
        const NOFOLLOW = 1 << 17;
        ///
        const LARGEFILE = 1 << 19;
        /// 是否是目录
        const DIR = 1 << 21;
    }
}

impl OpenFlags {
    /// 获得文件的读/写权限
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}
