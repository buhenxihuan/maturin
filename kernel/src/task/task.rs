//! 用户程序的数据及状态信息
//! 一个 TaskControlBlock 包含了一个任务(或进程)的所有信息

#![deny(missing_docs)]

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use alloc::string::String;
use lock::Mutex;
use core::slice::Iter;

use crate::loaders::{ElfLoader, parse_user_app};
use crate::memory::{MemorySet, Pid, new_memory_set_for_task};
use crate::memory::{VirtAddr, PTEFlags};
use crate::trap::TrapContext;
use crate::signal::{Signals, global_register_signals};
use crate::file::{FdManager, check_file_exists};
use crate::timer::get_time;
use crate::constants::{USER_STACK_OFFSET, NO_PARENT};
use crate::arch::get_cpu_id;

use super::{TaskContext, KernelStack};
use super::__move_to_context;

/// 任务控制块，包含一个用户程序的所有状态信息，但不包括与调度有关的信息。
/// 默认在TCB的外层对其的访问不会冲突，所以外部没有用锁保护，内部的 mutex 仅用来提供可变性
/// 
/// 目前来说，TCB外层可能是调度器或者 CpuLocal：
/// 1. 如果它在调度器里，则 Scheduler 内部不会修改它，且从 Scheduler 里取出或者放入 TCB 是由调度器外部的 Mutex 保护的；
/// 2. 如果它在 CpuLocal 里，则同时只会有一个核可以访问它，也不会冲突。
pub struct TaskControlBlock {
    /// 用户程序的内核栈，内部包含申请的内存空间
    /// 因为 struct 内部保存了页帧 Frame，所以 Drop 这个结构体时也会自动释放这段内存
    pub kernel_stack: KernelStack,
    /// 进程 id
    pub pid: Pid,
    /// 信号量相关信息。
    /// 因为发送信号是通过 pid/tid 查找的，因此放在 inner 中一起调用时更容易导致死锁
    pub signals: Arc<Mutex<Signals>>,
    /// 任务的状态信息
    pub inner: Mutex<TaskControlBlockInner>,
}

/// 任务控制块的可变部分
pub struct TaskControlBlockInner {
    /// 用户程序当前的工作目录
    /// - 注意 dir[0] == '.' ，如以 ./ 开头时代表根目录，以 "./abc/" 开头代表根目录下的abc目录。
    /// 这样处理是因为 open_file 时先打开文件所在目录，它的实现是先打开根目录，再从根目录找相对路径
    pub dir: String,
    /// 父进程的 pid。
    /// - 因为拿到 Pid 代表“拥有”这个 id 且 Drop 时会自动释放，所以此处用 usize 而不是 Pid。
    /// - 又因为它可能会在父进程结束时被修改为初始进程，所以是可变的。
    pub ppid: usize,
    /// 进程开始运行的时间
    pub start_tick: usize,
    /// 用户堆的堆顶。
    /// 用户堆和用户栈共用空间，反向增长，即从 USER_STACK_OFFSET 开始往上增加。
    /// 本来不应该由内存记录的，但 brk() 系统调用要用
    pub user_heap_top: usize,
    /// 任务执行状态
    pub task_status: TaskStatus,
    /// 上下文信息，用于切换，包含所有必要的寄存器
    /// 实际在第一次初始化时还包含了用户程序的入口地址和用户栈
    pub task_cx: TaskContext,
    /// 任务的内存段(内含页表)，同时包括用户态和内核态
    pub vm: MemorySet,
    /// 父进程
    pub parent: Option<Weak<TaskControlBlock>>,
    /// 子进程
    pub children: Vec<Arc<TaskControlBlock>>,
    /// sys_exit 时输出的值
    pub exit_code: i32,
    /// 管理进程的所有文件描述符
    pub fd_manager: FdManager,
}

impl TaskControlBlock {
    /// 从用户程序名生成 TCB，其中文件名默认为 args[0]
    /// 
    /// 在目前的实现下，如果生成 TCB 失败，只有以下情况：
    /// 1. 找不到文件名所对应的文件
    /// 2. 或者 loader 解析失败
    /// 
    /// 才返回 None，其他情况下生成失败会 Panic。
    /// 因为上面这两种情况是用户输入可能带来的，要把结果反馈给用户程序；
    /// 而其他情况(如 pid 分配失败、内核栈分配失败)是OS自己出了问题，应该停机。
    /// 
    /// 目前只有初始进程(/task/mod.rs: ORIGIN_USER_PROC) 直接通过这个函数初始化，
    /// 其他进程应通过 fork / exec 生成
    pub fn from_app_name(app_dir: &str, ppid: usize, args: Vec<String>) -> Option<Self> {
        if args.len() < 1 { // 需要至少有一项指定文件名
            return None
        }
        let app_name_string: String = args[0].clone();
        let app_name = app_name_string.as_str();
        if !check_file_exists(app_dir, app_name) {
            return None
        }
        // 新建页表，包含内核段
        let mut vm = new_memory_set_for_task().unwrap();
        // 找到用户名对应的文件，将用户地址段信息插入页表和 VmArea
        parse_user_app(app_dir, app_name, &mut vm, args)
            .map(|(user_entry, user_stack)| {
            //println!("user MemorySet {:#x?}", vm);
            // 初始化内核栈，它包含关于进入用户程序的所有信息
            let kernel_stack = KernelStack::new().unwrap();
            //kernel_stack.print_info();
            let pid = Pid::new().unwrap();
            println!("pid = {}", pid.0);
            let stack_top = kernel_stack.push_first_context(TrapContext::app_init_context(user_entry, user_stack));
            let signals = Arc::new(Mutex::new(Signals::new()));
            global_register_signals(pid.0, signals.clone());
            TaskControlBlock {
                kernel_stack: kernel_stack,
                pid: pid,
                signals: signals,
                inner: Mutex::new(TaskControlBlockInner {
                    dir: String::from(app_dir),
                    ppid: ppid,
                    start_tick: get_time(),
                    user_heap_top: USER_STACK_OFFSET,
                    task_cx: TaskContext::goto_restore(stack_top),
                    task_status: TaskStatus::Ready,
                    vm: vm,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_manager: FdManager::new(),
                }),
            }
        }).ok()
        
    }
    /// 从 fork 系统调用初始化一个TCB，并设置子进程对用户程序的返回值为0。
    /// 
    /// 参数 user_stack 为是否指定用户栈地址。如为 None，则沿用同进程的栈，否则使用该地址。由用户保证这个地址是有效的。
    /// 
    /// 这里只把父进程内核栈栈底的第一个 TrapContext 复制到子进程，
    /// 所以**必须保证对这个函数的调用是来自用户异常中断，而不是内核异常中断**。因为只有这时内核栈才只有一层 TrapContext。
    pub fn from_fork(self: &Arc<TaskControlBlock>, user_stack: Option<usize>) -> Arc<Self> {
        //println!("start fork");
        let mut inner = self.inner.lock();
        // 与 new 方法不同，这里从父进程的 MemorySet 生成子进程的
        let mut vm = inner.vm.copy_as_fork().unwrap(); 
        let kernel_stack = KernelStack::new().unwrap();
        // 与 new 方法不同，这里从父进程的 TrapContext 复制给子进程
        let mut trap_context = TrapContext::new();
        unsafe { trap_context = *self.kernel_stack.get_first_context(); }
        // 手动设置返回值为0，这样两个进程返回用户时除了返回值以外，都是完全相同的
        trap_context.set_a0(0);
        // 设置用户栈
        if let Some(user_stack_pos) = user_stack {
            trap_context.set_sp(user_stack_pos);
            //println!("sepc {:x} stack {:x}", trap_context.sepc, trap_context.get_sp());
        }
        let stack_top = kernel_stack.push_first_context(trap_context);
        
        let pid = Pid::new().unwrap();
        let dir = String::from(&inner.dir[..]);
        let ppid = self.pid.0;
        // 注意虽然 fork 之后信号模块的值不变，但两个进程已经完全分离了，对信号的修改不会联动
        // 所以不能只复制 Arc，要复制整个模块的值
        let new_signals = Arc::new(Mutex::new(self.signals.lock().clone()));
        // 但是存入全局表中的 signals 是只复制指针
        global_register_signals(pid.0, new_signals.clone());
        let new_tcb = Arc::new(TaskControlBlock {
            pid: pid,
            kernel_stack: kernel_stack,
            signals: new_signals,
            inner: {
                Mutex::new(TaskControlBlockInner {
                    dir: dir,
                    ppid: ppid,
                    start_tick: get_time(),
                    user_heap_top: USER_STACK_OFFSET,
                    task_cx: TaskContext::goto_restore(stack_top),
                    task_status: TaskStatus::Ready,
                    vm: vm,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_manager: inner.fd_manager.copy_all()
                })
            },
        });
        inner.children.push(new_tcb.clone());
        //println!("end fork");
        new_tcb
    }
    /// 从 exec 系统调用修改当前TCB，**默认新的用户程序与当前程序在同路径下**：
    /// 1. 从 ELF 文件中生成新的 MemorySet 替代当前的
    /// 2. 修改内核栈栈底的第一个 TrapContext 为新的用户程序的入口
    /// 3. 将传入的 args 作为用户程序执行时的参数
    /// 
    /// 如找不到对应的用户程序，则不修改当前进程且返回 False。
    /// 
    /// 注意 exec 不会清空用户程序执行的时间
    pub fn exec(&self, app_name: &str, args: Vec<String>) -> bool {
        let mut inner = self.inner.lock();
        if !check_file_exists(inner.dir.as_str(), app_name) {
            return false
        }
        // 清空用户堆
        inner.user_heap_top = USER_STACK_OFFSET;
        // 清空 MemorySet 中用户段的地址
        inner.vm.clear_user_and_save_kernel();
        // 清空信号模块
        self.signals.lock().clear();
        // 如果用户程序调用时没有参数，则手动加上程序名作为唯一的参数
        // 需要这个调整，是因为用户库(/user下)使用了 rCore 的版本，
        // 里面的 user_shell 调用 exec 时会加上程序名作为 args 的第一个参数
        // 但是其他函数调用 exec 时只会传入空的 args (包括初始进程)
        // 为了鲁棒性考虑，此处不修改用户库，而是手动分别这两种情况
        let args = if args.len() == 0 { vec![String::from(app_name)] } else { args };
        
        for i in 0..args.len() {
            info!("[cpu {}] args[{}] = '{}'", get_cpu_id(), i, args[i]);
        }
        
        // 然后把新的信息插入页表和 VmArea
        let dir = String::from(&inner.dir[..]);
        parse_user_app(dir.as_str(), app_name, &mut inner.vm, args)
            .map(|(user_entry, user_stack)| {
            // 修改完 MemorySet 映射后要 flush 一次
            inner.vm.flush_tlb();
            //println!("user vm {:#x?}", inner.vm);
            // argc 和 argv 存在用户栈顶，而按用户库里的实现是需要放在 a0 和 a1 寄存器中，所以这里手动取出
            let argc = unsafe {*(user_stack as *const usize)};
            let argv = unsafe {((user_stack as *const usize).add(1)) as usize};
            //println!("argc {} argv0 {:x}", argc, argv0);
            // 此处实际上覆盖了 kernel_stack 中原有的 TrapContext，内部用 unsafe 规避了此处原本应有的 mut
            let stack_top = self.kernel_stack.push_first_context(TrapContext::app_exec_context(user_entry, user_stack, argc, argv));
            inner.task_cx = TaskContext::goto_restore(stack_top);
            
            
            //let trap_context = unsafe {*self.kernel_stack.get_first_context() };
            //println!("sp = {:x}, entry = {:x}, sstatus = {:x}", trap_context.x[2], trap_context.sepc, trap_context.sstatus.bits()); 
        }).is_ok()
    }
    /// 映射一段内存地址到文件或设备。
    /// 
    /// anywhere 选项指示是否可以映射到任意位置，一般与 `MAP_FIXED` 关联。
    /// 如果 anywhere=true，则将 start 视为 hint
    pub fn mmap(&self, start: VirtAddr, end: VirtAddr, flags: PTEFlags, data: &[u8], anywhere: bool) -> Option<usize> {
        //info!("start {} , end {}, data.len {}", start, end, data.len());
        if end - start < data.len() {
           None
        } else {
            self.inner.lock().vm.push_with_data(start, end, flags, data, anywhere).ok()
        }
    }
    /// 取消一段内存地址映射
    pub fn munmap(&self, start: VirtAddr, end: VirtAddr) -> bool {
        self.inner.lock().vm.pop(start, end).is_ok()
    }
    /// 修改任务状态
    pub fn set_status(&self, new_status: TaskStatus) {
        let mut inner = self.inner.lock();
        inner.task_status = new_status;
    }
    /// 输入 exit code
    pub fn set_exit_code(&self, exit_code: i32) {
        let mut inner = self.inner.lock();
        inner.exit_code = exit_code;
    }
    /// 读取任务状态
    pub fn get_status(&self) -> TaskStatus {
        let inner = self.inner.lock();
        inner.task_status
    }
    /// 读取任务上下文
    pub fn get_task_cx_ptr(&self) -> *const TaskContext {
        let inner = self.inner.lock();
        &inner.task_cx
    }
    /// 获取 pid 的值，不会转移或释放 Pid 的所有权
    pub fn get_pid_num(&self) -> usize {
        self.pid.0
    }
    /// 获取 ppid 的值
    pub fn get_ppid(&self) -> usize {
        let ppid = self.inner.lock().ppid;
        if ppid == NO_PARENT { 
            1 
        } else {
            ppid
        }
    }
    /// 获取程序开始时间
    pub fn get_start_tick(&self) -> usize {
        self.inner.lock().start_tick
    }
    /// 获取用户堆顶地址
    pub fn get_user_heap_top(&self) -> usize {
        self.inner.lock().user_heap_top
    }
    /// 重新设置堆顶地址，如成功则返回设置后的堆顶地址，否则保持不变，并返回之前的堆顶地址。
    /// 新地址需要在用户栈内，并且不能碰到目前的栈
    pub fn set_user_heap_top(&self, new_top: usize) -> usize {
        let user_sp = unsafe { (*self.kernel_stack.get_first_context()).get_sp() };
        let mut inner = self.inner.lock();
        if new_top >= USER_STACK_OFFSET && new_top < user_sp {
            inner.user_heap_top = new_top;
            new_top
        } else {
            inner.user_heap_top
        }
    }
    /// 如果当前进程已是运行结束，则获取其 exit_code，否则返回 None
    pub fn get_code_if_exit(&self) -> Option<i32> {
        let inner = self.inner.try_lock()?; 
        match inner.task_status {
            TaskStatus::Zombie => Some(inner.exit_code),
            _ => None
        }
    }
    
}

/// 任务执行状态
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// 还未初始化
    UnInit,
    /// 已初始化但还未执行，可以被任意一个核执行
    Ready, 
    /// 正在被一个核执行
    Running, 
    /// 进程在用户端已退出，但内核端还有些工作要处理，例如把它的所有子进程交给初始进程
    Dying,
    /// 僵尸进程，已退出，但其资源还在等待回收
    Zombie,
    /// 已执行完成
    Exited,
}
