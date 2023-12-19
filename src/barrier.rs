use crate::lamellae::{AllocationType, Lamellae, LamellaeRDMA};
use crate::lamellar_arch::LamellarArchRT;
// use crate::lamellar_memregion::{SharedMemoryRegion,RegisteredMemoryRegion};
use crate::memregion::MemoryRegion; //, RTMemoryRegionRDMA, RegisteredMemoryRegion};
use crate::scheduler::Scheduler;
// use rand::prelude::SliceRandom;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

const DISSEMINATION_FACTOR: usize = 2;

pub(crate) struct Barrier {
    my_pe: usize, // global pe id
    num_pes: usize,
    n: usize, // dissemination factor
    num_rounds: usize,
    pub(crate) arch: Arc<LamellarArchRT>,
    pub(crate) _scheduler: Arc<Scheduler>,
    lamellae: Arc<Lamellae>,
    barrier_cnt: AtomicUsize,
    barrier_buf: Vec<MemoryRegion<usize>>,
    send_buf: Option<MemoryRegion<usize>>,
    panic: Arc<AtomicU8>,
}

impl Barrier {
    pub(crate) fn new(
        my_pe: usize,
        global_pes: usize,
        lamellae: Arc<Lamellae>,
        arch: Arc<LamellarArchRT>,
        scheduler: Arc<Scheduler>,
        panic: Arc<AtomicU8>,
    ) -> Barrier {
        let num_pes = arch.num_pes;
        let mut n = std::env::var("LAMELLAR_BARRIER_DISSEMNATION_FACTOR")
            .unwrap_or(DISSEMINATION_FACTOR.to_string())
            .parse::<usize>()
            .unwrap();
        let num_rounds = if n > 1 && num_pes > 2 {
            ((num_pes as f64).log2() / (n as f64).log2()).ceil() as usize
        } else {
            n = 1;
            (num_pes as f64).log2() as usize
        };
        let (buffs, send_buf) = if let Ok(_my_index) = arch.team_pe(my_pe) {
            if num_pes > 1 {
                let alloc = if global_pes == arch.num_pes {
                    AllocationType::Global
                } else {
                    let mut pes = arch.team_iter().collect::<Vec<usize>>();
                    pes.sort();
                    AllocationType::Sub(pes)
                };
                // println!("creating barrier {:?}", alloc);
                let mut buffs = vec![];
                for _ in 0..n {
                    buffs.push(MemoryRegion::new(
                        num_rounds,
                        lamellae.clone(),
                        alloc.clone(),
                    ));
                }

                let send_buf = MemoryRegion::new(1, lamellae.clone(), alloc);

                unsafe {
                    for buff in &buffs {
                        for elem in buff.as_mut_slice().expect("Data should exist on PE") {
                            *elem = 0;
                        }
                    }
                    for elem in send_buf.as_mut_slice().expect("Data should exist on PE") {
                        *elem = 0;
                    }
                }
                (buffs, Some(send_buf))
            } else {
                (vec![], None)
            }
        } else {
            (vec![], None)
        };

        let bar = Barrier {
            my_pe: my_pe,
            num_pes: num_pes,
            n: n,
            num_rounds: num_rounds,
            arch: arch,
            _scheduler: scheduler,
            lamellae: lamellae,
            barrier_cnt: AtomicUsize::new(1),
            barrier_buf: buffs,
            send_buf: send_buf,
            panic: panic,
        };
        // bar.print_bar();
        bar
    }

    fn print_bar(&self) {
        if let Some(send_buf) = &self.send_buf {
            let buffs = self
                .barrier_buf
                .iter()
                .map(|b| b.as_slice().unwrap())
                .collect::<Vec<_>>();
            println!(
                "[{:?}] [LAMELLAR BARRIER] {:?} {:?} {:?}",
                self.my_pe,
                buffs,
                send_buf.as_slice().expect("Data should exist on PE"),
                self.barrier_cnt.load(Ordering::SeqCst)
            );
        }
    }
    pub(crate) fn barrier(&self) {
        // println!("in barrier");
        let mut s = Instant::now();
        if self.panic.load(Ordering::SeqCst) == 0 {
            if let Some(send_buf) = &self.send_buf {
                if let Ok(my_index) = self.arch.team_pe(self.my_pe) {
                    let send_buf_slice = unsafe {
                        // im the only thread (remote or local) that can write to this buff
                        send_buf.as_mut_slice().expect("Data should exist on PE")
                    };

                    let barrier_id = self.barrier_cnt.fetch_add(1, Ordering::SeqCst);
                    send_buf_slice[0] = barrier_id;
                    let barrier_slice = &[barrier_id];
                    // println!("barrier_id = {:?}", barrier_id);

                    for round in 0..self.num_rounds {
                        for i in 1..=self.n {
                            let team_send_pe =
                                (my_index + i * (self.n + 1).pow(round as u32)) % self.num_pes;
                            if team_send_pe != my_index {
                                let send_pe = self.arch.single_iter(team_send_pe).next().unwrap();
                                // println!(
                                //     "[ {:?} {:?}] round: {:?}  i: {:?} sending to [{:?} ({:?}) ] id: {:?} buf {:?}",
                                //     self.my_pe,
                                //     my_index,
                                //     round,
                                //     i,
                                //     send_pe,
                                //     team_send_pe,
                                //     send_buf_slice,
                                //     unsafe {
                                //         self.barrier_buf[i - 1]
                                //             .as_mut_slice()
                                //             .expect("Data should exist on PE")
                                //     }
                                // );
                                unsafe {
                                    self.barrier_buf[i - 1].put_slice(
                                        send_pe,
                                        round,
                                        barrier_slice,
                                    );
                                    //safe as we are the only ones writing to our index
                                }
                            }
                        }
                        for i in 1..=self.n {
                            let team_recv_pe = ((my_index as isize
                                - (i as isize * (self.n as isize + 1).pow(round as u32) as isize))
                                as isize)
                                .rem_euclid(self.num_pes as isize)
                                as isize;
                            let recv_pe =
                                self.arch.single_iter(team_recv_pe as usize).next().unwrap();
                            if team_recv_pe as usize != my_index {
                                // println!(
                                //     "[{:?} ] recv from [{:?} ({:?}) ] id: {:?} buf {:?}",
                                //     self.my_pe,
                                //     recv_pe,
                                //     team_recv_pe,
                                //     send_buf_slice,
                                //     unsafe {
                                //         self.barrier_buf[i - 1]
                                //             .as_mut_slice()
                                //             .expect("Data should exist on PE")
                                //     }
                                // );
                                unsafe {
                                    //safe as  each pe is only capable of writing to its own index
                                    while self.barrier_buf[i - 1]
                                        .as_mut_slice()
                                        .expect("Data should exist on PE")[round]
                                        < barrier_id
                                    {
                                        if s.elapsed().as_secs_f64() > *crate::DEADLOCK_TIMEOUT {
                                            println!("[WARNING] Potential deadlock detected.\n\
                                        Barrier is a collective operation requiring all PEs associated with the distributed object to enter the barrier call.\n\
                                        Please refer to https://docs.rs/lamellar/latest/lamellar/index.html?search=barrier for more information\n\
                                        Note that barriers are often called internally for many collective operations, including constructing new LamellarTeams, LamellarArrays, and Darcs, as well as distributed iteration\n\
                                        A full list of collective operations is found at https://docs.rs/lamellar/latest/lamellar/index.html?search=collective\n\
                                        The deadlock timeout can be set via the LAMELLAR_DEADLOCK_TIMEOUT environment variable, the current timeout is {} seconds\n\
                                        To view backtrace set RUST_LIB_BACKTRACE=1\n\
                                        {}",*crate::DEADLOCK_TIMEOUT,std::backtrace::Backtrace::capture());

                                            println!(
                                                "[{:?}, {:?}] round: {:?} i: {:?} teamsend_pe: {:?} team_recv_pe: {:?} recv_pe: {:?}",
                                                self.my_pe,
                                                my_index,
                                                round,
                                                i,
                                                (my_index + i * (self.n + 1).pow(round as u32))
                                                    % self.num_pes,
                                                team_recv_pe,
                                                recv_pe
                                            );
                                            self.print_bar();
                                            s = Instant::now();
                                        }
                                        self.lamellae.flush();
                                        std::thread::yield_now();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// impl Drop for Barrier {
//     fn drop(&mut self) {
//         //println!("dropping barrier");
//         // println!("arch: {:?}",Arc::strong_count(&self.arch));
//         //println!("dropped barrier");
//     }
// }
