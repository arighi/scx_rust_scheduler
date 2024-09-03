// Copyright (c) Andrea Righi <andrea.righi@linux.dev>

// This software may be used and distributed according to the terms of the
// GNU General Public License version 2.

//! # FIFO Linux kernel scheduler that runs in user-space
//!
//! ## Overview
//!
//! This is a fully functional FIFO scheduler for the Linux kernel that operates in user-space and
//! it is 100% implemented in Rust.
//!
//! The scheduler is designed to serve as a simple template for developers looking to implement
//! more advanced scheduling policies.
//!
//! It is based on `scx_rustland_core`, a framework that is specifically designed to simplify the
//! creation of user-space schedulers, leveraging the Linux kernel's `sched_ext` feature (a
//! technology that allows to implement schedulers in BPF).
//!
//! The `scx_rustland_core` crate offers an abstraction layer over `sched_ext`, enabling developers
//! to write schedulers in Rust without needing to interact directly with low-level kernel or BPF
//! internal details.
//!
//! ## scx_rustland_core API
//!
//! ### struct `BpfScheduler`
//!
//! The `BpfScheduler` struct is the core interface for interacting with `sched_ext` via BPF.
//!
//! - **Initialization**:
//!   - `BpfScheduler::init()` registers the scheduler and initializes the BPF component.
//!
//! - **Task Management**:
//!   - `dequeue_task()`: Consume a task that wants to run
//!   - `select_cpu(pid: i32, prev_cpu: i32, flags: u64)`: Select an idle CPU for a task
//!   - `dispatch_task(task: &DispatchedTask)`: Dispatch a task
//!
//! - **Completion Notification**:
//!   - `notify_complete(nr_pending: u64)` Give control to the BPF component and report the number
//!      of tasks that are still pending (this function can sleep)
//!
//! ## Task scheduling workflow
//!
//!  +----------------------------------------------+
//!  | // task is received                          |
//!  | task = BpfScheduler.dequeue_task()           |
//!  +----------------------+-----------------------+
//!                        |
//!                        v
//!  +----------------------------------------------+
//!  | // Create a new task to dispatch             |
//!  | dispatched_task = DispatchedTask::new(&task);|
//!  +---------------------+------------------------+
//!                        |
//!                        v
//!  +----------------------------------------------+
//!  | // Pick an idle CPU for the task             |
//!  | cpu = BpfScheduler.select_cpu()              |
//!  +---------------------+------------------------+
//!                        |
//!                        v
//!       +----------------+-----------------+
//!       | cpu >= 0                         | cpu < 0
//!       v                                  v
//!  +----------------------------+    +-----------------------------+
//!  | // Assign the idle CPU     |    | // Run on first CPU avail   |
//!  +----------------------------+    +-----------------------------+
//!        |                                 |
//!        +---------------+-----------------+
//!                        |
//!                        v
//!  +----------------------------------------------+
//!  | // Dispatch the task                         |
//!  | BpfScheduler.dispatch_task(dispatched_task)  |
//!  +---------------------+------------------------+
//!                        |
//!                        v
//!  +----------------------------------------------+
//!  | // Notify BPF component                      |
//!  | BpfScheduler.notify_complete()               |
//!  +----------------------------------------------+

mod bpf_skel;
pub use bpf_skel::*;
pub mod bpf_intf;

mod bpf;
use bpf::*;

use scx_utils::UserExitInfo;

use libbpf_rs::OpenObject;

use std::collections::VecDeque;
use std::mem::MaybeUninit;

use anyhow::Result;

// Maximum time (in milliseconds) that a task can run before it is re-enqueued into the scheduler.
const SLICE_NS: u64 = 5_000_000;

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,            // Connector to the sched_ext BPF backend
    task_queue: VecDeque<QueuedTask>, // FIFO queue used to store tasks
}

impl<'a> Scheduler<'a> {
    fn init(open_object: &'a mut MaybeUninit<OpenObject>) -> Result<Self> {
        let bpf = BpfScheduler::init(
            open_object,
            0,     // exit_dump_len (buffer size of exit info, 0 = default)
            false, // partial (false = include all tasks)
            false, // debug (false = debug mode off)
        )?;
        Ok(Self {
            bpf,
            task_queue: VecDeque::new(),
        })
    }

    /// Consume all tasks that are ready to run.
    fn consume_tasks(&mut self) {
        // Each task contains the following details:
        //
        // pub struct QueuedTask {
        //     pub pid: i32,              // pid that uniquely identifies a task
        //     pub cpu: i32,              // CPU where the task is running
        //     pub sum_exec_runtime: u64, // Total cpu time in nanoseconds
        //     pub weight: u64,           // Task static priority in the range [1..10000]
        //                                // (default weight is 100)
        // }
        //
        // Although the FIFO scheduler doesn't use these fields, they can provide valuable data for
        // implementing more sophisticated scheduling policies.
        while let Ok(Some(task)) = self.bpf.dequeue_task() {
            self.task_queue.push_back(task);
        }
    }

    /// Dispatch next task ready to run.
    fn dispatch_next_task(&mut self) {
        if let Some(task) = self.task_queue.pop_front() {
            // Create a new task to be dispatched, derived from the received enqueued task.
            //
            // pub struct DispatchedTask {
            //     pub pid: i32,      // pid that uniquely identifies a task
            //     pub cpu: i32,      // target CPU selected by the scheduler
            //     pub flags: u64,    // special dispatch flags (RL_CPU_ANY = dispatch on the first
            //                        // CPU available)
            //     pub slice_ns: u64, // time slice in nanoseconds assigned to the task
            //                        // (0 = use default)
            // }
            //
            // The dispatched task's information are pre-populated from the QueuedTask and they can
            // be modified before dispatching it via self.bpf.dispatch_task().
            let mut dispatched_task = DispatchedTask::new(&task);

            // Decide where the task needs to run (target CPU).
            //
            // A call to select_cpu() will return the most suitable idle CPU for the task,
            // considering its previously used CPU.
            let cpu = self.bpf.select_cpu(task.pid, task.cpu, 0);
            if cpu >= 0 {
                // Run the task on the idle CPU that we just found.
                dispatched_task.cpu = cpu;
            } else {
                // If there's no idle CPU available, simply run on the first one available.
                dispatched_task.flags |= RL_CPU_ANY;
            }

            // Decide for how long the task needs to run (time slice); if not specified
            // SCX_SLICE_DFL will be used by default.
            dispatched_task.slice_ns = SLICE_NS;

            // Dispatch the task on the target CPU.
            self.bpf.dispatch_task(&dispatched_task).unwrap();

            // Notify the BPF component of the number of pending tasks and immediately give a
            // chance to run to the dispatched task.
            self.bpf.notify_complete(self.task_queue.len() as u64);
        }
    }

    /// Scheduler main loop.
    fn run(&mut self) -> Result<UserExitInfo> {
        println!("Rust scheduler is enabled (CTRL+c to exit)");
        while !self.bpf.exited() {
            // Consume all tasks before dispatching any.
            self.consume_tasks();

            // Dispatch one task from the global queue.
            self.dispatch_next_task();

            // If all tasks have been dispatched notify the BPF component that all tasks have been
            // scheduled / dispatched, with 0 remaining pending tasks.
            if self.task_queue.is_empty() {
                self.bpf.notify_complete(0);
            }
        }

        println!("Rust scheduler is disabled");
        self.bpf.shutdown_and_report()
    }
}

fn main() -> Result<()> {
    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object)?;
        if !sched.run()?.should_restart() {
            break;
        }
    }

    Ok(())
}
