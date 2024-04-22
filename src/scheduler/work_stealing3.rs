use crate::env_var::config;
use crate::scheduler::{LamellarExecutor, SchedulerStatus};
use crate::MAIN_THREAD;

//use tracing::*;

use async_task::{Builder, Runnable};
use core_affinity::CoreId;
use futures_util::Future;
use rand::prelude::*;
use std::panic;
use std::pin::Pin;
use std::process;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
//, Weak};
use std::thread;

use crossbeam::deque::{Injector, Stealer, Worker};
use thread_local::ThreadLocal;

static TASK_ID: AtomicUsize = AtomicUsize::new(0);

lazy_static! {
    static ref WORK_Q: ThreadLocal<Worker<Runnable<usize>>> = ThreadLocal::new();
}

#[derive(Debug)]
pub(crate) struct WorkStealingThread {
    imm_inj: Arc<Injector<Runnable<usize>>>,
    work_inj: Arc<Injector<Runnable<usize>>>,
    work_stealers: Vec<Stealer<Runnable<usize>>>,
    // work_q: Arc<HashMap<ThreadId, Worker<Runnable<usize>>>>,
    work_flag: Arc<AtomicU8>,
    status: Arc<AtomicU8>,
    panic: Arc<AtomicU8>,
}

impl WorkStealingThread {
    //#[tracing::instrument(skip_all)]
    fn run(
        worker: WorkStealingThread,
        work_q: Worker<Runnable<usize>>,
        active_cnt: Arc<AtomicUsize>,
        // num_tasks: Arc<AtomicUsize>,
        id: CoreId,
    ) -> thread::JoinHandle<()> {
        let builder = thread::Builder::new().name("worker_thread".into());
        builder
            .spawn(move || {
                // println!("TestSchdulerWorker thread running {:?} core: {:?}", std::thread::current().id(), id);
                // let _span = trace_span!("WorkStealingThread::run");
                core_affinity::set_for_current(id);
                let work_q = WORK_Q.get_or(|| work_q);
                active_cnt.fetch_add(1, Ordering::SeqCst);
                let mut rng = rand::thread_rng();
                let t = rand::distributions::Uniform::from(0..worker.work_stealers.len());
                let mut timer = std::time::Instant::now();
                while worker.panic.load(Ordering::SeqCst) == 0
                    && (
                        worker.status.load(Ordering::SeqCst) == SchedulerStatus::Active as u8
                            || !(work_q.is_empty()
                                && worker.work_inj.is_empty()
                                && worker.imm_inj.is_empty())
                        // || num_tasks.load(Ordering::SeqCst) > 1
                    )
                {
                    let omsg = if !worker.imm_inj.is_empty() {
                        worker.imm_inj.steal().success()
                    } else {
                        work_q.pop().or_else(|| {
                            if worker.work_flag.compare_exchange(
                                0,
                                1,
                                Ordering::SeqCst,
                                Ordering::Relaxed,
                            ) == Ok(0)
                            {
                                let ret = worker.work_inj.steal_batch_and_pop(work_q).success();
                                worker.work_flag.store(0, Ordering::SeqCst);
                                ret
                            } else {
                                let pe = t.sample(&mut rng);
                                if worker.work_stealers[pe].len() > 100 {
                                    worker.work_stealers[t.sample(&mut rng)].steal().success()
                                } else {
                                    None
                                }
                            }
                        })
                    };

                    if let Some(runnable) = omsg {
                        if worker.status.load(Ordering::SeqCst) == SchedulerStatus::Finished as u8
                            && timer.elapsed().as_secs_f64() > config().deadlock_timeout
                        {
                            println!("runnable {:?}", runnable);
                            println!(
                                "work_q size {:?} work inj size {:?}", // num_tasks {:?}",
                                work_q.len(),
                                worker.work_inj.len(),
                                // num_tasks.load(Ordering::SeqCst)
                            );
                            timer = std::time::Instant::now();
                        }
                        runnable.run();
                    }
                    if worker.status.load(Ordering::SeqCst) == SchedulerStatus::Finished as u8
                        && timer.elapsed().as_secs_f64() > config().deadlock_timeout
                        && (work_q.len() > 0 || worker.work_inj.len() > 0)
                    {
                        println!(
                            "work_q size {:?} work inj size {:?} ", // num_tasks {:?}",
                            work_q.len(),
                            worker.work_inj.len(),
                            // num_tasks.load(Ordering::SeqCst)
                        );
                        timer = std::time::Instant::now();
                    }
                    std::thread::yield_now();
                }
                active_cnt.fetch_sub(1, Ordering::SeqCst);
                // println!("TestSchdulerWorker thread shutting down");
            })
            .unwrap()
    }
}

#[derive(Debug)]
pub(crate) struct WorkStealing3 {
    max_num_threads: usize,
    threads: Vec<thread::JoinHandle<()>>,
    imm_inj: Arc<Injector<Runnable<usize>>>,
    work_inj: Arc<Injector<Runnable<usize>>>,
    work_stealers: Vec<Stealer<Runnable<usize>>>,
    work_flag: Arc<AtomicU8>,
    status: Arc<AtomicU8>,
    active_cnt: Arc<AtomicUsize>,
    panic: Arc<AtomicU8>,
}

impl LamellarExecutor for WorkStealing3 {
    fn submit_task<F>(&self, task: F)
    where
        F: Future + Send + 'static,
        F::Output: Send,
    {
        // trace_span!("submit_task").in_scope(|| {
        let work_inj = self.work_inj.clone();
        let schedule = move |runnable| {
            // if thread::current().id() == *MAIN_THREAD {
            work_inj.push(runnable);
            // } else {
            //     WORK_Q.get().unwrap().push(runnable);
            // }
        };
        let (runnable, task) = Builder::new()
            .metadata(TASK_ID.fetch_add(1, Ordering::Relaxed))
            .spawn(move |_task_id| async move { task.await }, schedule);

        runnable.schedule();
        task.detach();
        // });
    }

    fn submit_io_task<F>(&self, task: F)
    where
        F: Future + Send + 'static,
        F::Output: Send,
    {
        // trace_span!("submit_task").in_scope(|| {

        let work_inj = self.work_inj.clone();
        let schedule = move |runnable| {
            if thread::current().id() == *MAIN_THREAD {
                work_inj.push(runnable);
            } else {
                WORK_Q.get().unwrap().push(runnable);
            }
        };
        let (runnable, task) = Builder::new()
            .metadata(TASK_ID.fetch_add(1, Ordering::Relaxed))
            .spawn(move |_task_id| async move { task.await }, schedule);
        runnable.schedule();
        task.detach();
        // });
    }

    fn submit_immediate_task<F>(&self, task: F)
    where
        F: Future + Send + 'static,
        F::Output: Send,
    {
        // trace_span!("submit_task").in_scope(|| {
        let imm_inj = self.imm_inj.clone();
        let schedule = move |runnable| imm_inj.push(runnable);
        let (runnable, task) = Builder::new()
            .metadata(TASK_ID.fetch_add(1, Ordering::Relaxed))
            .spawn(move |_task_id| async move { task.await }, schedule);

        runnable.run(); //try to run immediately
        task.detach();
        // });
    }

    fn block_on<F: Future>(&self, fut: F) -> F::Output {
        // trace_span!("block_on").in_scope(|| {
        let work_inj = self.work_inj.clone();
        let schedule = move |runnable| work_inj.push(runnable);
        let (runnable, mut task) = unsafe {
            Builder::new()
                .metadata(TASK_ID.fetch_add(1, Ordering::Relaxed))
                .spawn_unchecked(move |_task_id| async move { fut.await }, schedule)
        };
        let waker = runnable.waker();
        runnable.run(); //try to run immediately
        while !task.is_finished() {
            self.exec_task(); //try to execute another task while this one is not ready
        }
        let cx = &mut Context::from_waker(&waker);
        if let Poll::Ready(output) = Pin::new(&mut task).poll(cx) {
            output
        } else {
            println!(
                "[{:?}] work stealing block on failed --  task id{:?}",
                std::thread::current().id(),
                task.metadata()
            );
            panic!("task not ready");
        }
        // })
    }

    //#[tracing::instrument(skip_all)]
    fn shutdown(&self) {
        while self.panic.load(Ordering::SeqCst) == 0 && self.active_cnt.load(Ordering::Relaxed) > 0
        {
            //num active threads
            self.exec_task();
        }
    }

    //#[tracing::instrument(skip_all)]
    fn force_shutdown(&self) {
        // println!("work stealing shuting down {:?}", self.status());

        // println!("work stealing shuting down {:?}",self.status());
        let my_id = std::thread::current().id();
        if self.threads.iter().any(|e| e.thread().id() == my_id) {
            self.active_cnt.fetch_sub(1, Ordering::SeqCst); // I paniced so I wont actually decrement
        } else {
            while self.active_cnt.load(Ordering::Relaxed) > 0 {
                //num active threads
                self.exec_task();
            }
        }
        // println!(
        //     "work stealing shut down {:?} {:?} {:?}",
        //     self.status(),
        //     self.active_cnt.load(Ordering::Relaxed),
        //     self.active_cnt.load(Ordering::Relaxed)
        // );
    }

    //#[tracing::instrument(skip_all)]
    fn exec_task(&self) {
        let mut rng = rand::thread_rng();
        let t = rand::distributions::Uniform::from(0..self.work_stealers.len());
        let ret = if !self.imm_inj.is_empty() {
            self.imm_inj.steal().success()
        } else {
            if self
                .work_flag
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed)
                == Ok(0)
            {
                let ret = self.work_inj.steal().success();
                self.work_flag.store(0, Ordering::SeqCst);
                ret
            } else {
                // self.work_stealers[t.sample(&mut rng)].steal().success()
                None
            }
        };
        if let Some(runnable) = ret {
            runnable.run();
        } else {
            std::thread::yield_now();
        }
    }

    fn set_max_workers(&mut self, num_workers: usize) {
        self.max_num_threads = num_workers;
    }

    fn num_workers(&self) -> usize {
        self.max_num_threads
    }
}

impl WorkStealing3 {
    pub(crate) fn new(
        num_workers: usize,
        status: Arc<AtomicU8>,
        panic: Arc<AtomicU8>,
    ) -> WorkStealing3 {
        // println!("new work stealing queue");
        let mut ws = WorkStealing3 {
            max_num_threads: std::cmp::max(1, num_workers - 1),
            threads: Vec::new(),
            imm_inj: Arc::new(Injector::new()),
            work_inj: Arc::new(Injector::new()),
            work_stealers: Vec::new(),
            work_flag: Arc::new(AtomicU8::new(0)),
            status: status,
            active_cnt: Arc::new(AtomicUsize::new(0)),
            panic: panic,
        };
        ws.init();
        ws
    }
    // #[tracing::instrument(skip_all)]
    fn init(&mut self) {
        let mut work_workers: std::vec::Vec<crossbeam::deque::Worker<Runnable<usize>>> = vec![];
        for _i in 0..self.max_num_threads {
            let work_worker: crossbeam::deque::Worker<Runnable<usize>> =
                crossbeam::deque::Worker::new_fifo();
            self.work_stealers.push(work_worker.stealer());
            work_workers.push(work_worker);
        }

        let orig_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            // invoke the default handler and exit the process
            orig_hook(panic_info);
            process::exit(1);
        }));
        let core_ids = match core_affinity::get_core_ids() {
            Some(core_ids) => core_ids,
            None => {
                vec![core_affinity::CoreId { id: 0 }]
            }
        };
        // println!("core_ids: {:?}",core_ids);
        println!("num threads: {} {}", self.max_num_threads, core_ids.len());
        for i in 0..self.max_num_threads {
            let work_worker = work_workers.pop().unwrap();
            let worker = WorkStealingThread {
                imm_inj: self.imm_inj.clone(),
                work_inj: self.work_inj.clone(),
                work_stealers: self.work_stealers.clone(),
                work_flag: self.work_flag.clone(),
                status: self.status.clone(),
                panic: self.panic.clone(),
            };
            self.threads.push(WorkStealingThread::run(
                worker,
                work_worker,
                self.active_cnt.clone(),
                // self.num_tasks.clone(),
                core_ids[i % core_ids.len()],
            ));
        }
        while self.active_cnt.load(Ordering::SeqCst) != self.threads.len() {
            std::thread::yield_now();
        }
    }
}

impl Drop for WorkStealing3 {
    //when is this called with respect to world?
    //#[tracing::instrument(skip_all)]
    fn drop(&mut self) {
        // println!("dropping work stealing");
        while let Some(thread) = self.threads.pop() {
            if thread.thread().id() != std::thread::current().id() {
                let _res = thread.join();
            }
        }
        // println!("WorkStealing Scheduler Dropped");
    }
}
