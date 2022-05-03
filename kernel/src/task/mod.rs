//! 任务管理
//!
//! 全局常量 TASK_MANAGER 在初始化过程中导入所有用户程序的数据，并分别保存在一个 TaskControlBlock 中
//! 所有的 TaskControlBlock 都放在 Arc<Mutex<TaskManagerInner>> 中，
//! 每个核需要切换任务时都需要拿到这个锁，且从调度开始到结束**必须一直持有**这个锁
//!

#![deny(missing_docs)]

use lazy_static::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{Ordering, AtomicUsize};
use lock::Mutex;

mod context;
mod switch;

#[allow(clippy::module_inception)]
mod task;

use crate::constants::{CPU_NUM, EMPTY_TASK};
use crate::error::{OSResult, OSError};
use crate::loader::{get_num_app, get_app_data, init_app_cx_by_entry_and_stack};
use crate::loaders::ElfLoader;
use crate::memory::{MemorySet, new_memory_set_for_task, VirtAddr, PTEFlags};
use crate::arch::get_cpu_id;
use switch::__switch;
use task::{TaskControlBlock, TaskStatus};

pub use context::TaskContext;

/// 任务管理器，管理所有用户程序
pub struct TaskManager {
    /// 任务数
    num_app: usize,
    /// 可变部分用锁保护，每次只能有一个核在访问
    inner: Arc<Mutex<TaskManagerInner>>,
    /// 已经完成任务的核数。不放在inner里检查是为了避免干扰其他核调度
    /// 也不放在panic里是因为：
    ///     默认情况下，只要一个核panic，OS必须停机，方便debug
    ///     而任务调度是特殊情况，所有核调度完才panic，所以在调度里写
    finished_core_cnt: AtomicUsize,
}

/// 任务管理器的可变部分
pub struct TaskManagerInner {
    /// 每个用户程序的所有信息放在一个 TCB 中
    tasks: Vec<TaskControlBlock>,
    /// 对每个核，存储当前正在运行哪个任务
    /// 我们约定每个核只修改自己对应的 usize，所以对这个数组的访问其实是不会冲突的
    /// 不过它是 TaskManager 中的可变部分，为了省去调用时候的 mut 姑且放在 inner 里
    current_task_at_cpu: [usize; CPU_NUM],
}

lazy_static! {
    /// Global variable: TASK_MANAGER
    pub static ref TASK_MANAGER: TaskManager = {
        // 通过 loader 获取用户程序数
        let num_app = get_num_app();
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        // 初始化每个用户程序的 TCB
        for i in 0..num_app {
            // 新建页表，包含内核段
            let mut vm = new_memory_set_for_task().unwrap();
            // 获取用户库编译链接好的(elf 格式的)用户数据，然后插入页表和 VmArea
            let raw_data = get_app_data(i);
            let loader = ElfLoader::new(raw_data).unwrap();
            let args = vec![String::from(".")];
            let (user_entry, user_stack) = loader.init_vm(&mut vm, args).unwrap();
            // 初始化内核栈，它包含关于进入用户程序的所有信息
            let trap_cx_ptr_in_kernel_stack = init_app_cx_by_entry_and_stack(i, user_entry, user_stack);
            tasks.push(TaskControlBlock{
                task_cx: TaskContext::goto_restore(trap_cx_ptr_in_kernel_stack),
                task_status: TaskStatus::Ready,
                vm: vm,
            });
        }
        TaskManager {
            num_app,
            inner: Arc::new(Mutex::new(TaskManagerInner {
                    tasks,
                    current_task_at_cpu: [0; CPU_NUM],
                })),
            finished_core_cnt: AtomicUsize::new(0),
        }
    };
}

impl TaskManager {
    /// 运行第一个任务，如没有可运行的任务则无限 loop {} 直到所有核完成所有任务。
    /// 与 run_next_task 的区别在于需要构造一个空的 TaskContext 用于“被切换”
    fn run_first_task(&self) {
        let mut inner = self.inner.lock();
        let cpu_id = get_cpu_id();
        // 这里初始化num_app-1，是为了第一次启动的时候通过run_next_task
        // 中的(current + 1..current + self.num_app + 1)来选择任务，也就是相当于从0开始选择
        // 而在多核环境下，一个核启动的第一个应用不一定是task[0]
        inner.current_task_at_cpu[cpu_id] = self.num_app - 1;

        if let Some(next) = self.find_next_task(&inner, cpu_id) { // 如果找到任务则进入执行
            println!("[cpu {}] running task {}", cpu_id, next);
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task_at_cpu[cpu_id] = next;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            // 切换页表。所有 TCB 中的页表在内核中的地址映射必须相同，否则换页表的时候pc可能跑飞
            unsafe {inner.tasks[next].vm.activate(); }
            // 在 switch 换内核栈之前必须先 drop 掉当前拿着的锁
            drop(inner);
            let mut _unused = TaskContext::zero_init();
            unsafe {
                __switch(&mut _unused as *mut TaskContext, next_task_cx_ptr);
            }
            // 回到 trap 然后返回用户
        } else { // 否则停机
            if inner.current_task_at_cpu[cpu_id] != EMPTY_TASK {
                inner.current_task_at_cpu[cpu_id] = EMPTY_TASK;
            }
            drop(inner);
            self.wait_for_all_tasks_completed();
            unreachable!();
        }
    }

    /// 从当前 cpu 正在运行的任务 id 开始，寻找下一个可以被运行的任务，如能找到，则返回任务 id 。
    /// 该函数需要 inner 的锁，但因为“切换任务”是一个完整的原子操作，而“寻找下一个任务”不是，所以它不会自己申请 inner 的锁，需要参数传入
    fn find_next_task(&self, inner: &TaskManagerInner, cpu_id: usize) -> Option<usize> {
        //let inner = self.inner.lock();
        let current = inner.current_task_at_cpu[cpu_id];
        //如果当前cpu已停机，则不再接受新用户程序
        if self.num_app == 0 || inner.current_task_at_cpu[cpu_id] == EMPTY_TASK {
            None
        } else {
            (current + 1..current + self.num_app + 1)
                .map(|id| id % self.num_app)
                .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
        }
    }

    /// 切换下一个任务，将当前任务的状态置为 new_status_for_current，
    /// 如有可运行的任务则将其状态置为 Running 并进入执行，
    /// 如没有可运行的任务则无限 loop {} 直到所有核完成所有任务
    fn run_next_task(&self, new_status_for_current: TaskStatus) {
        //println!("[cpu {}] into next", get_cpu_id());
        let mut inner = self.inner.lock();
        //println!("[cpu {}] get lock", get_cpu_id());
        let cpu_id = get_cpu_id();
        //在寻找下一个任务前先修改current状态。这一步需要在inner.lock()保护下进行
        let current = inner.current_task_at_cpu[cpu_id];
        inner.tasks[current].task_status = new_status_for_current;

        if let Some(next) = self.find_next_task(&inner, cpu_id) { // 如果找到任务则进入执行
            println!("[cpu {}] leaving task {}", cpu_id, current);
            println!("[cpu {}] running task {}", cpu_id, next);
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task_at_cpu[cpu_id] = next;
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            // 切换页表。所有 TCB 中的页表在内核中的地址映射必须相同，否则换页表的时候pc可能跑飞
            unsafe {inner.tasks[next].vm.activate(); }
            extern "C" {
                fn _num_app();
            }
            // println!("_num_app {:x}", _num_app as usize);
            // 在 switch 换内核栈之前必须先 drop 掉当前拿着的锁
            drop(inner);
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // 回到 trap 然后返回用户
        } else {
            if inner.current_task_at_cpu[cpu_id] != EMPTY_TASK {
                println!("[cpu {}] leaving task {}", cpu_id, inner.current_task_at_cpu[cpu_id]);
                inner.current_task_at_cpu[cpu_id] = EMPTY_TASK;
            }
            drop(inner);
            self.wait_for_all_tasks_completed();
            unreachable!();
        }
    }
    /*
    fn get_mut_vm_now(&self, inner: &'_ TaskManagerInner) -> Option<&'_ mut MemorySet> {
        let cpu_id = get_cpu_id();
        let task_now = inner.current_task_at_cpu[cpu_id];
        if task_now < get_num_app() && task_now >= 0 {
            Some(&mut inner.tasks[task_now].vm)
        } else {
            None
        }
    }
    */
    /// 处理用户程序的缺页异常
    fn handle_user_page_fault(&self, vaddr: VirtAddr, access_flags: PTEFlags) -> OSResult {
        //println!("into task pf");
        let mut inner = self.inner.lock();
        let cpu_id = get_cpu_id();
        let task_now = inner.current_task_at_cpu[cpu_id];

        //extern "C" {fn _num_app();}
        //println!("into task pf {} {:x}", task_now, _num_app as usize );
        if /* task_now < get_num_app()  && */ task_now >= 0 {
            inner.tasks[task_now].vm.handle_page_fault(vaddr, access_flags)
        } else {
            Err(OSError::Task_NoTrapHandler)
        }
    }

    /// 标记当前核已完成所有任务，返回目前已完成任务的核数(包括当前核)
    fn mark_finish(&self) -> usize {
        self.finished_core_cnt.fetch_add(1, Ordering::Acquire) + 1
    }

    /// 若所有核完成任务，则panic退出；
    /// 否则一直 loop {} 等待
    fn wait_for_all_tasks_completed(&self) {
        if self.mark_finish() == CPU_NUM {
            panic!("All applications completed!");
        }
        println!("[cpu {}] is idle now.", get_cpu_id());
        loop {
        }
    }
}

/// 运行第一个任务
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// 切换下一个任务，将当前任务的状态置为 new_status_for_current，
fn run_next_task(new_status_for_current: TaskStatus) {
    TASK_MANAGER.run_next_task(new_status_for_current);
}

/// 暂停当前任务并切换到下一个任务。
/// 一般来自时钟中断或 sys_yield
pub fn suspend_current_and_run_next() {
    //mark_current_suspended();
    run_next_task(TaskStatus::Ready);
}

/// 退出当前任务并切换到下一个任务
pub fn exit_current_and_run_next() {
    //mark_current_exited();
    run_next_task(TaskStatus::Exited);
}

/// 处理用户程序的缺页异常。
/// 不需要指定是哪个用户程序，函数内部会根据调用函数的核的 cpu_id 去查找
pub fn handle_user_page_fault(vaddr: VirtAddr, access_flags: PTEFlags) -> OSResult {
    TASK_MANAGER.handle_user_page_fault(vaddr, access_flags)
}
