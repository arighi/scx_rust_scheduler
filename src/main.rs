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
//!   - `dequeue_task()`: Consume a task that wants to run, returns a QueuedTask object
//!   - `select_cpu(pid: i32, prev_cpu: i32, flags: u64)`: Select an idle CPU for a task
//!   - `dispatch_task(task: &DispatchedTask)`: Dispatch a task
//!
//! - **Completion Notification**:
//!   - `notify_complete(nr_pending: u64)` Give control to the BPF component and report the number
//!      of tasks that are still pending (this function can sleep)
//!
//! Each task received from dequeue_task() contains the following:
//!
//! struct QueuedTask {
//!     pub pid: i32,              // pid that uniquely identifies a task
//!     pub cpu: i32,              // CPU previously used by the task
//!     pub sum_exec_runtime: u64, // Total cpu time in nanoseconds
//!     pub weight: u64,           // Task priority in the range [1..10000] (default is 100)
//! }
//!
//! Each task dispatched using dispatch_task() contains the following:
//!
//! struct DispatchedTask {
//!     pub pid: i32,      // pid that uniquely identifies a task
//!     pub cpu: i32,      // target CPU selected by the scheduler
//!     pub flags: u64,    // special dispatch flags (RL_CPU_ANY = dispatch on the first
//!                        // CPU available)
//!     pub slice_ns: u64, // time slice in nanoseconds assigned to the task
//!                        // (0 = use default)
//!     pub vtime: u64,    // this value can be used to send the task's vruntime or deadline
//!                        // directly to the underlying BPF dispatcher
//! }

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

// Maximum time slice (in nanoseconds) that a task can use before it is re-enqueued.
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
        while let Ok(Some(task)) = self.bpf.dequeue_task() {
            self.task_queue.push_back(task);
        }
    }

    /// Dispatch tasks that are ready to run.
    fn dispatch_tasks(&mut self) {
        // Get the amount of tasks that are waiting to be scheduled.
        let nr_waiting = self.task_queue.len() as u64;

        while let Some(task) = self.task_queue.pop_front() {
            // Create a new task to be dispatched from the received enqueued task.
            let mut dispatched_task = DispatchedTask::new(&task);

            // Decide where the task needs to run (pick a target CPU).
            //
            // A call to select_cpu() will return the most suitable idle CPU for the task,
            // prioritizing its previously used CPU (available in task.cpu).
            let cpu = self.bpf.select_cpu(task.pid, task.cpu, 0);
            if cpu >= 0 {
                // Run the task on the idle CPU that we just found.
                dispatched_task.cpu = cpu;
            } else {
                // No idle CPU available, simply run the task on the first CPU available.
                dispatched_task.flags |= RL_CPU_ANY;
            }

            // Determine the task's time slice: assign value inversely proportional to the number
            // of tasks waiting to be scheduled.
            dispatched_task.slice_ns = SLICE_NS / (nr_waiting + 1);

            // Dispatch the task.
            self.bpf.dispatch_task(&dispatched_task).unwrap();
        }
    }

    /// Scheduler main loop.
    fn run(&mut self) -> Result<UserExitInfo> {
        println!("Rust scheduler is enabled (CTRL+c to exit)");
        while !self.bpf.exited() {
            // Consume all the tasks that want to run.
            self.consume_tasks();

            // Dispatch all tasks from the global queue.
            self.dispatch_tasks();

            // Notify the BPF component that all the pending tasks have been dispatched.
            //
            // This function will put the scheduler to sleep, until another task needs to run.
            self.bpf.notify_complete(0);
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
