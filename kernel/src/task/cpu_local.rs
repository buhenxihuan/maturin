//! 每个核当前正在运行的任务及上下文信息

#![deny(missing_docs)]

use alloc::vec::Vec;
use alloc::sync::Arc;
use core::cell::{RefCell, RefMut};
use lock::Mutex;
use lazy_static::*;

use crate::constants::{CPU_ID_LIMIT, IS_TEST_ENV, NO_PARENT};
use crate::error::{OSResult, OSError};
use crate::trap::TrapContext;
use crate::signal::global_logoff_signals;
use crate::memory::{VirtAddr, PTEFlags, enable_kernel_page_table};
use crate::file::show_testcase_result;
use crate::arch::get_cpu_id;

use super::{__switch, __move_to_context};
use super::{fetch_task_from_scheduler, push_task_to_scheduler};
use super::{ORIGIN_USER_PROC};
use super::{TaskContext, TaskControlBlock, TaskStatus};

/// 每个核当前正在运行的任务及上下文信息。
/// 注意，如果一个核没有运行在任何任务上，那么它会回到 idle_task_cx 的上下文，而这里的栈就是启动时的栈。
/// 启动时的栈空间在初始化内核 MemorySet 与页表时有留出 shadow page，也即如果在核空闲时不断嵌套异常中断导致溢出，
/// 会在 trap 中进入 StorePageFault，然后panic终止系统
pub struct CpuLocal {
    /// 这个核当前正在运行的用户程序
    current: Option<Arc<TaskControlBlock>>,
    /// 无任务时的上下文，实际存的是启动时的上下文(其中的栈是 entry.S 中的 idle_stack)
    idle_task_cx: TaskContext,
}

impl CpuLocal {
    ///Create an empty Processor
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
        }
    }
    ///Get mutable reference to `idle_task_cx`
    fn get_idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }
    ///Get current task in moving semanteme
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }
    ///Get current task in cloning semanteme
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

lazy_static! {
    /// 所有 CPU 的上下文信息
    pub static ref CPU_CONTEXTS: Vec<Mutex<CpuLocal>> = {
        let mut cpu_contexts: Vec<Mutex<CpuLocal>> = Vec::new();
        for i in 0..CPU_ID_LIMIT {
            cpu_contexts.push(Mutex::new(CpuLocal::new()));
        }
        cpu_contexts
    };
}

/// 开始执行用户程序
pub fn run_tasks() -> ! {
    let cpu_id = get_cpu_id();
    loop {
        if let Some(task) = fetch_task_from_scheduler() {
            let mut cpu_local = CPU_CONTEXTS[cpu_id].lock();
            //let mut task_inner = task.lock();
            let idle_task_cx_ptr = cpu_local.get_idle_task_cx_ptr();
            let next_task_cx_ptr = task.get_task_cx_ptr();
            task.set_status(TaskStatus::Running);

            //let pid = task.get_pid_num();
            //if pid == 2 { println!("[cpu {}] now running on pid = {}", cpu_id, pid);}
            //drop(task_inner);
            unsafe { task.inner.lock().vm.activate(); }
            cpu_local.current = Some(task);

            /*
            unsafe {
                println!("[cpu {}] idle task ctx ptr {:x}, next {:x}, ra = {:x} pid = {}", 
                    cpu_id, 
                    idle_task_cx_ptr as usize, 
                    next_task_cx_ptr as usize, 
                    (*next_task_cx_ptr).get_ra(),
                    cpu_local.current.as_ref().unwrap().get_pid_num());
                let t0 = (0xffff_ffff_8020_1234 as *const usize).read_volatile();
                let t1 = (0x1000 as *const usize).read_volatile();
                println!("t0 {:x}, t1 {:x}", t0, t1);
                //println!("{:#x?}", cpu_local.current.as_ref().unwrap().inner.lock().vm);
            }
            */
            
            // 切换前要手动 drop 掉引用
            drop(cpu_local);
            // 切换到用户程序执行
            unsafe {
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            }
            // 在上面的用户程序中，会执行 suspend_current_and_run_next() 或  exit_current_and_run_next(exit_code: i32)
            // 在其中会修改 current.task_status 和 exit_code，但任务本身还在被当前 CPU 占用，需要下面再将其插入队列或
            let mut cpu_local = CPU_CONTEXTS[cpu_id].lock();
            // 切换回只有内核的页表。在此之后就不能再访问该任务用户空间的内容
            enable_kernel_page_table();
            // 此时已切回空闲任务
            if let Some(task) = cpu_local.take_current() {
                // println!("[cpu {}] now leave pid = {}", cpu_id, task.get_pid_num());
                let status = task.get_status();
                match status {
                    TaskStatus::Ready => {
                        // 将暂停的用户程序塞回任务队列
                        push_task_to_scheduler(task);
                    }
                    TaskStatus::Dying => {
                        if !IS_TEST_ENV && task.get_pid_num() == 0 { // 这是初始进程，且不在测试环境
                            panic!("origin user proc exited, All applications completed.");
                        } else {
                            handle_zombie_task(&mut cpu_local, task);
                        }
                    }
                    _ => {
                        panic!("invalid task status when switched out");
                    }
                }
            } else {
                panic!("[cpu {}] CpuLocal: switched from empty task", get_cpu_id());
            }
            // 因为 task 是 task_current() 得到的，所以如果 task 不是 ORIGIN_USER_PROC，它在上面的 if 结束时就已经没有了 Arc 引用
            // 其内部的 Pid, MemorySet 等应在此时被 Drop
            drop(cpu_local);
        }
    }
}

/// 暂停当前用户程序，回到 idle 状态
pub fn suspend_current_task() {
    let cpu_id = get_cpu_id();
    let mut cpu_local = CPU_CONTEXTS[cpu_id].lock();
    let task = cpu_local.current().unwrap();
    //let task_inner = task.lock();
    task.set_status(TaskStatus::Ready);
    // let task = cpu_local.take_current_task(); 只有写好用户程序的内核栈、回到 idle 状态以后，才能把任务塞回队列里
    // add_task(task);
    let current_task_cx_ptr = task.get_task_cx_ptr() as *mut TaskContext;
    let idle_task_cx_ptr = cpu_local.get_idle_task_cx_ptr();
    //println!("idle task context ptr {:x}", idle_task_cx_ptr as usize);
    //drop(task_inner);
    drop(task);
    drop(cpu_local);
    // 切换回 run_tasks() 中
    unsafe {
        __switch(current_task_cx_ptr, idle_task_cx_ptr);
    }
}

/// 终止当前用户程序，回到 idle 状态
pub fn exit_current_task(exit_code: i32) {
    let cpu_id = get_cpu_id();
    let mut cpu_local = CPU_CONTEXTS[cpu_id].lock();
    let task = cpu_local.current().unwrap();
    // let task_inner = task.lock();
    task.set_status(TaskStatus::Dying);
    task.set_exit_code(exit_code);
    //println!("[cpu {}] pid {} exited with code {}", cpu_id, task.get_pid_num(), exit_code);
    let idle_task_cx_ptr = cpu_local.get_idle_task_cx_ptr();
    //println!("idle task context ptr {:x}", idle_task_cx_ptr as usize);
    //drop(task_inner);
    drop(task);
    drop(cpu_local);
    // 切换回 run_tasks() 中
    unsafe {
        __move_to_context(idle_task_cx_ptr);
    }
}

/// 通过 exec 系统调用，直接切换到新的用户进程
pub fn exec_new_task() {
    let cpu_id = get_cpu_id();
    let mut cpu_local = CPU_CONTEXTS[cpu_id].lock();
    let task = cpu_local.current().unwrap();
    //println!("user vm {:#x?}", task.inner.lock().vm);
    let current_task_cx_ptr = task.get_task_cx_ptr() as *mut TaskContext;
    drop(task);
    drop(cpu_local);
    unsafe {
        __move_to_context(current_task_cx_ptr);
    }   
}
/// 处理退出的进程：
/// 将它的子进程全部交给初始进程 ORIGIN_USER_PROC，然后标记当前进程的状态为 Zombie。
/// 这里会需要获取当前核正在运行的用户程序、ORIGIN_USER_PROC、所有子进程的锁。
/// 
/// 这里每修改一个子进程的 parent 指针，都要重新用 try_lock 拿子进程的锁和 ORIGIN_USER_PROC 的锁。
/// 
/// 如果不用 try_lock ，则可能出现如下的死锁情况：
/// 1. 当前进程和子进程都在这个函数里
/// 2. 当前进程拿到了 ORIGIN_USER_PROC 的锁，而子进程在函数开头等待 ORIGIN_USER_PROC 的锁
/// 3. 当前进程尝试修改子进程的 parent，但无法修改。因为子进程一直拿着自己的锁，它只是在等 ORIGIN_USER_PROC
/// 
/// 使用 try_lock 之后，如果出现多个父子进程竞争锁的情况，那么：
/// 1. 如果拿到 ORIGIN_USER_PROC 的锁的进程的子进程都没有在竞争这个锁，那么它一定可以顺利拿到自己的所有子进程的锁，并正常执行完成。
/// 2. 否则，它会因为无法拿到自己的某个子进程的锁而暂时放弃 ORIGIN_USER_PROC 的锁。
/// 
/// 因为进程之间的 parent/children 关系是一棵树，所以在任意时刻一定会有上述第一种情况的进程存在。
/// 所以卡在这个函数上的进程最终一定能以某种顺序依次执行完成，也就消除了死锁。
/// 
fn handle_zombie_task(cpu_local: &mut CpuLocal, task: Arc<TaskControlBlock>) {
    let mut tcb_inner = task.inner.lock();
    //let task_inner = task.lock();
    
    for child in tcb_inner.children.iter() {
        loop {
            // 这里把获取子进程的锁放在外层，是因为如果当前进程和子进程都在这个函数里，
            // 父进程可能拿到 start_proc 的锁，但一定拿不到 child 的锁。
            // 因为每个进程在进这个函数时都拿着自己的锁，所以此时只有子进程先执行完成，父进程才能继续执行。
            // 为了防止父进程反复抢 start_proc 的锁又不得不释放，所以把获取子进程的锁放在外层
            if let Some(mut child_inner) = child.inner.try_lock() {
                if tcb_inner.ppid == NO_PARENT || IS_TEST_ENV {
                    child_inner.ppid = NO_PARENT;
                } else if let Some(mut start_proc_tcb_inner) =  ORIGIN_USER_PROC.clone().inner.try_lock() {
                    child_inner.parent = Some(Arc::downgrade(&ORIGIN_USER_PROC));
                    child_inner.ppid = 0;
                    start_proc_tcb_inner.children.push(child.clone());
                    // 拿到锁并修改完成后，退到外层循环去修改下一个子进程
                    break;
                }
            }
            // 只要没拿到任意一个锁，就继续循环
        }
    }
    tcb_inner.children.clear();
    tcb_inner.task_status = TaskStatus::Zombie;
    // 通知全局表将 signals 删除
    global_logoff_signals(task.pid.0);
    // 在测试环境中时，手动检查退出时的 exit_code
    if IS_TEST_ENV {
        show_testcase_result(tcb_inner.exit_code);
    }
    /*
    // 释放用户段占用的物理页面
    // 如果这里不释放，等僵尸进程被回收时 MemorySet 被 Drop，也可以释放这些页面
    tcb_inner.vm.clear_user();
    */
}

/// 处理用户程序的缺页异常
pub fn handle_user_page_fault(vaddr: VirtAddr, access_flags: PTEFlags)  -> OSResult {
    let cpu_id = get_cpu_id();
    let cpu_local = CPU_CONTEXTS[cpu_id].lock();
    if let Some(task) = cpu_local.current() {
        task.inner.lock().vm.handle_page_fault(vaddr, access_flags)
    } else {
        Err(OSError::Task_NoTrapHandler)
    }
}

/// 获取当前核正在运行的进程的TCB。
/// 如果当前核没有任务，则返回 None
pub fn get_current_task() -> Option<Arc<TaskControlBlock>> {
    Some(CPU_CONTEXTS[get_cpu_id()].lock().current.as_ref()?.clone())
}

/// 处理所有信号
pub fn handle_signal() {
    
}
