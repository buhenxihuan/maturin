//! 系统调用编号

numeric_enum_macro::numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    /// 系统调用编号
    pub enum SyscallNo {
        UNKNOWN = usize::MAX, // 未识别的系统调用
        GETCWD = 17,
        EPOLL_CREATE = 20,
        EPOLL_CTL = 21,
        EPOLL_WAIT = 22,
        DUP = 23,
        DUP3 = 24,
        FCNTL64 = 25,
        IOCTL = 29,
        MKDIR = 34,
        UNLINKAT = 35,
        LINKAT = 37,
        UMOUNT = 39,
        MOUNT = 40,
        STATFS = 43,
        ACCESS = 48,
        CHDIR = 49,
        OPEN = 56,
        CLOSE = 57,
        PIPE = 59,
        GETDENTS64 = 61,
        LSEEK = 62,
        READ = 63,
        WRITE = 64,
        READV = 65,
        WRITEV = 66,
        PREAD = 67,
        SENDFILE64 = 71,
        PSELECT6 = 72,
        PPOLL = 73,
        READLINKAT = 78,
        FSTATAT = 79,
        FSTAT = 80,
        FSYNC = 82,
        UTIMENSAT = 88,
        EXIT = 93,
        EXIT_GROUP = 94,
        SET_TID_ADDRESS = 96,
        FUTEX = 98,
        NANOSLEEP = 101,
        GETITIMER = 102,
        SETITIMER = 103,
        CLOCK_GET_TIME = 113,
        SYSLOG = 116,
        YIELD = 124,
        KILL = 129,
        TKILL = 130,
        SIGACTION = 134,
        SIGPROCMASK = 135,
        SIGTIMEDWAIT = 137,
        SIGRETURN = 139,
        TIMES = 153,
        UNAME = 160,
        GETRUSAGE = 165,
        UMASK = 166,
        PRCTL = 167,
        GET_TIME_OF_DAY = 169,
        GETPID = 172,
        GETPPID = 173,
        GETUID = 174,
        GETEUID = 175,
        GETGID = 176,
        GETEGID = 177,
        GETTID = 178,
        SYSINFO = 179,
        SOCKET = 198,
        BIND = 200,
        LISTEN = 201,
        ACCEPT = 202,
        CONNECT = 203,
        GETSOCKNAME = 204,
        GETPEERNAME = 205,
        SENDTO = 206,
        RECVFROM = 207,
        SETSOCKOPT = 208,
        GETSOCKOPT = 209,
        SHUDOWN = 210,
        SENDMSG = 211,
        RECVMSG = 212,
        BRK = 214,
        MUNMAP = 215,
        CLONE = 220,
        EXECVE = 221,
        MMAP = 222,
        MPROTECT = 226,
        MSYNC = 227,
        MADVISE = 233,
        ACCEPT4 = 242,
        WAIT4 = 260,
        PRLIMIT64 = 261,
        RENAMEAT2 = 276,
        MEMBARRIER = 283,
    }
}
