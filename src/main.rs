// Copyright (c) Andrea Righi <andrea.righi@linux.dev>

// This software may be used and distributed according to the terms of the
// GNU General Public License version 2.

mod bpf_skel;
pub use bpf_skel::*;
pub mod bpf_intf;

mod bpf;
use bpf::*;

use scx_utils::UserExitInfo;

use libbpf_rs::OpenObject;

use std::collections::HashMap;
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::time::SystemTime;

use anyhow::Result;

// Maximum time (in nanoseconds) that a task can run before it is re-enqueued into the scheduler.
const SLICE_NS: u64 = 5_000_000;

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,            // Connector to the sched_ext BPF backend
    task_queue: VecDeque<QueuedTask>, // Global queue used to temporarily store tasks
    sum_exec_runtime: HashMap<i32, u64>,
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
            sum_exec_runtime: HashMap::new(),
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
            let mut dispatched_task = DispatchedTask::new(&task);

            // Decide where the task needs to run (pick a target CPU).
            //
            // A call to select_cpu() will return the most suitable idle CPU for the task,
            // prioritizing its previously used CPU (available in task.cpu).
            //
            // If a CPU is not specified the task will be dispatched to the previously used CPU.
            let cpu = self.bpf.select_cpu(task.pid, task.cpu, 0);
            if cpu >= 0 {
                // Run the task on the idle CPU that we just found.
                dispatched_task.cpu = cpu;
            } else {
                // No idle CPU available, simply run the task on the first CPU available.
                dispatched_task.flags |= RL_CPU_ANY;
            }

            // Determine the task's runtime (time slice): assign a time slice that is inversely
            // proportional to the number of tasks waiting to be scheduled.
            //
            // If a time slice is not specified, a default value will be used (20ms).
            dispatched_task.slice_ns = SLICE_NS / (nr_waiting + 1);

            // Evaluate task's previously used time slice looking at the total exec run-time.
            let task_runtime = *self.sum_exec_runtime.get(&task.pid).unwrap_or(&0);
            let task_slice = (task.sum_exec_runtime - task_runtime).min(SLICE_NS);
            self.sum_exec_runtime
                .insert(task.pid, task.sum_exec_runtime);

            // Set task's deadline based on current time and weighted used time slice.
            let deadline = Self::now() + task_slice * 100 / task.weight;
            dispatched_task.vtime = deadline;

            // Dispatch the task.
            self.bpf.dispatch_task(&dispatched_task).unwrap();
        }
    }

    // Return current timestamp in ns.
    fn now() -> u64 {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        ts.as_nanos() as u64
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
