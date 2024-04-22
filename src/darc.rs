//! Distributed Atomic Reference Counter-- a distributed extension of an [`Arc`][std::sync::Arc] called a [Darc][crate::darc].
//! The atomic reference counter, [`Arc`][std::sync::Arc], is a backbone of safe
//! concurrent programming in Rust, and, in particular, *shared ownership*.
//!
//! The `Darc` provides a similar abstraction within a *distributed* environment.
//! - `Darc`'s have global lifetime tracking and management, meaning that the pointed to objects remain valid and accessible
//!   as long as one reference exists on any PE.
//! - Inner mutability is disallowed by default. If you need to mutate through a Darc use [`Mutex`][std::sync::Mutex], [`RwLock`][std::sync::RwLock], or one of the [`Atomic`][std::sync::atomic]
//! types. Alternatively you can also use a [`LocalRwDarc`] or [`GlobalRwDarc`].
//!
//! `Darc`'s are intended to be passed via active messages.
//! - They allow distributed
//!   accesss to and manipulation of generic Rust objects.  The inner object can exist
//!   on the Rust heap or in a registered memory region.
//! - They are instantiated in registered memory regions.
//! # Examples
//!```
//! use lamellar::active_messaging::prelude::*;
//! use lamellar::darc::prelude::*;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use std::sync::Arc;
//!
//! #[lamellar::AmData(Clone)]
//! struct DarcAm {
//!     counter: Darc<AtomicUsize>, //each pe has a local atomicusize
//! }
//!
//! #[lamellar::am]
//! impl LamellarAm for DarcAm {
//!     async fn exec(self) {
//!         self.counter.fetch_add(1, Ordering::SeqCst); //this only updates atomic on the executing pe
//!     }
//!  }
//!
//! fn main(){
//!     let world = LamellarWorldBuilder::new().build();
//!     let my_pe = world.my_pe();
//!     let num_pes = world.num_pes();
//!     let darc_counter = Darc::new(&world, AtomicUsize::new(0)).unwrap();
//!     world.exec_am_all(DarcAm {counter: darc_counter.clone()});
//!     darc_counter.fetch_add(my_pe, Ordering::SeqCst);
//!     world.wait_all(); // wait for my active message to return
//!     world.barrier(); //at this point all updates will have been performed
//!     assert_eq!(darc_counter.load(Ordering::SeqCst),num_pes+my_pe); //NOTE: the value of darc_counter will be different on each PE
//! }
///```
use async_lock::RwLock;
use core::marker::PhantomData;
use futures_util::future::join_all;
use serde::{Deserialize, Deserializer};
use std::cmp::PartialEq;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
// use std::time::Instant;

// //use tracing::*;

use crate::active_messaging::{AMCounters, AmHandle, RemotePtr};
use crate::barrier::Barrier;
use crate::env_var::config;
use crate::lamellae::{AllocationType, Backend, LamellaeComm, LamellaeRDMA};
use crate::lamellar_team::{IntoLamellarTeam, LamellarTeamRT};
use crate::lamellar_world::LAMELLAES;
use crate::{IdError, LamellarEnv, LamellarTeam};

#[doc(hidden)]
pub mod prelude;

pub(crate) mod local_rw_darc;
pub use local_rw_darc::LocalRwDarc;

pub(crate) mod global_rw_darc;
use global_rw_darc::DistRwLock;
pub use global_rw_darc::GlobalRwDarc;

static DARC_ID: AtomicUsize = AtomicUsize::new(0);

#[repr(u8)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub(crate) enum DarcMode {
    Dropped,
    Darc,
    LocalRw,
    GlobalRw,
    UnsafeArray,
    ReadOnlyArray,
    // LocalOnlyArray,
    // AtomicArray,
    GenericAtomicArray,
    NativeAtomicArray,
    LocalLockArray,
    GlobalLockArray,
}

#[lamellar_impl::AmDataRT(Debug)]
struct FinishedAm {
    cnt: usize,
    src_pe: usize,
    inner_addr: usize, //cant pass the darc itself cause we cant handle generics yet in lamellarAM...
}

#[lamellar_impl::rt_am]
impl LamellarAM for FinishedAm {
    async fn exec() {
        // println!("in finished! {:?}",self);
        let inner = unsafe { &*(self.inner_addr as *mut DarcInner<()>) }; //we dont actually care about the "type" we wrap here, we just need access to the meta data for the darc
                                                                          // inner.team().print_cnt();
        inner.dist_cnt.fetch_sub(self.cnt, Ordering::SeqCst);
    }
}

#[doc(hidden)]
#[repr(C)]
pub struct DarcInner<T> {
    id: usize,
    my_pe: usize,           // with respect to LamellarArch used to create this object
    num_pes: usize,         // with respect to LamellarArch used to create this object
    local_cnt: AtomicUsize, // cnt of times weve cloned for local access
    total_local_cnt: AtomicUsize,
    weak_local_cnt: AtomicUsize, // cnt of times weve cloned for local access with a weak reference
    dist_cnt: AtomicUsize,       // cnt of times weve cloned (serialized) for distributed access
    total_dist_cnt: AtomicUsize,
    ref_cnt_addr: usize, // array of cnts for accesses from remote pes
    total_ref_cnt_addr: usize,
    mode_addr: usize,
    mode_ref_cnt_addr: usize,
    mode_barrier_addr: usize,
    // mode_barrier_rounds: usize,
    barrier: *mut Barrier,
    am_counters: *const AMCounters,
    team: *const LamellarTeamRT,
    item: *const T,
    drop: Option<fn(&mut T)>,
    valid: AtomicBool,
}
unsafe impl<T> Send for DarcInner<T> {} //we cant create DarcInners without going through the Darc interface which enforces  Sync+Send
unsafe impl<T> Sync for DarcInner<T> {} //we cant create DarcInners without going through the Darc interface which enforces  Sync+Send

/// Distributed atomic reference counter
///
/// The atomic reference counter, [`Arc`][std::sync::Arc], is a backbone of safe
/// concurrent programming in Rust, and, in particular, *shared ownership*.
///
/// The `Darc` provides a similar abstraction within a *distributed* environment.
/// - `Darc`'s have global lifetime, meaning that the pointed to objects remain valid and accessible
///   as long as one reference exists on any PE.
/// - Inner mutability is disallowed by default. If you need to mutate through a Darc use [`Mutex`][std::sync::Mutex], [`RwLock`][std::sync::RwLock], or one of the [`Atomic`][std::sync::atomic]
/// types. Alternatively you can also use a [`LocalRwDarc`] or [`GlobalRwDarc`].
///
/// `Darc`'s are intended to be passed via active messages.
/// - They allow distributed
///   accesss to and manipulation of generic Rust objects.  The inner object can exist
///   on the Rust heap or in a registered memory region.
/// - They are instantiated in registered memory regions.
///
/// # Examples
///```
/// use lamellar::active_messaging::prelude::*;
/// use lamellar::darc::prelude::*;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::sync::Arc;
///
/// #[lamellar::AmData(Clone)]
/// struct DarcAm {
///     counter: Darc<AtomicUsize>, //each pe has a local atomicusize
/// }
///
/// #[lamellar::am]
/// impl LamellarAm for DarcAm {
///     async fn exec(self) {
///         self.counter.fetch_add(1, Ordering::SeqCst); //this only updates atomic on the executing pe
///     }
///  }
///
/// fn main(){
///     let world = LamellarWorldBuilder::new().build();
///     let my_pe = world.my_pe();
///     let num_pes = world.num_pes();
///     let darc_counter = Darc::new(&world, AtomicUsize::new(0)).unwrap();
///     world.exec_am_all(DarcAm {counter: darc_counter.clone()});
///     darc_counter.fetch_add(my_pe, Ordering::SeqCst);
///     world.wait_all(); // wait for my active message to return
///     world.barrier(); //at this point all updates will have been performed
///     assert_eq!(darc_counter.load(Ordering::SeqCst),num_pes+my_pe); //NOTE: the value of darc_counter will be different on each PE
/// }
///```
pub struct Darc<T: 'static> {
    inner: *mut DarcInner<T>,
    src_pe: usize,
}
unsafe impl<T: Sync + Send> Send for Darc<T> {}
unsafe impl<T: Sync + Send> Sync for Darc<T> {}

impl<T> LamellarEnv for Darc<T> {
    fn my_pe(&self) -> usize {
        self.inner().my_pe
    }
    fn num_pes(&self) -> usize {
        self.inner().num_pes
    }
    fn num_threads_per_pe(&self) -> usize {
        self.inner().team().num_threads_per_pe()
    }
    fn world(&self) -> Arc<LamellarTeam> {
        // println!("Darc world");
        self.inner().team().world()
    }
    fn team(&self) -> Arc<LamellarTeam> {
        // println!("Darc team");
        self.inner().team().team()
    }
}

impl<T: 'static> serde::Serialize for Darc<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        __NetworkDarc::from(self).serialize(serializer)
    }
}

impl<'de, T: 'static> Deserialize<'de> for Darc<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ndarc: __NetworkDarc = Deserialize::deserialize(deserializer)?;
        Ok(ndarc.into())
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct WeakDarc<T: 'static> {
    inner: *mut DarcInner<T>,
    src_pe: usize,
}
unsafe impl<T: Send> Send for WeakDarc<T> {}
unsafe impl<T: Sync> Sync for WeakDarc<T> {}

impl<T> WeakDarc<T> {
    pub fn upgrade(&self) -> Option<Darc<T>> {
        let inner = unsafe { &*self.inner };
        inner.local_cnt.fetch_add(1, Ordering::SeqCst);
        inner.total_local_cnt.fetch_add(1, Ordering::SeqCst);
        if inner.valid.load(Ordering::SeqCst) {
            Some(Darc {
                inner: self.inner,
                src_pe: self.src_pe,
            })
        } else {
            inner.local_cnt.fetch_sub(1, Ordering::SeqCst);
            None
        }
    }
}

impl<T> Drop for WeakDarc<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };
        // println!("dropping weak darc\n {:?}", inner);
        inner.weak_local_cnt.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<T> Clone for WeakDarc<T> {
    fn clone(&self) -> Self {
        let inner = unsafe { &*self.inner };
        inner.weak_local_cnt.fetch_add(1, Ordering::SeqCst);
        WeakDarc {
            inner: self.inner,
            src_pe: self.src_pe,
        }
    }
}

impl<T> crate::active_messaging::DarcSerde for Darc<T> {
    fn ser(&self, num_pes: usize, darcs: &mut Vec<RemotePtr>) {
        // println!("darc ser");
        self.serialize_update_cnts(num_pes);
        darcs.push(RemotePtr::NetworkDarc(self.clone().into()));
        // self.print();
    }
    fn des(&self, cur_pe: Result<usize, IdError>) {
        // println!("darc des");
        match cur_pe {
            Ok(_) => {
                self.deserialize_update_cnts();
            }
            Err(err) => {
                panic!("can only access darcs within team members ({:?})", err);
            }
        }
        // self.print();
    }
}

impl<T> DarcInner<T> {
    fn team(&self) -> Pin<Arc<LamellarTeamRT>> {
        unsafe {
            Arc::increment_strong_count(self.team);
            Pin::new_unchecked(Arc::from_raw(self.team))
        }
    }

    fn am_counters(&self) -> Arc<AMCounters> {
        unsafe {
            Arc::increment_strong_count(self.am_counters);
            Arc::from_raw(self.am_counters)
        }
    }

    fn inc_pe_ref_count(&self, pe: usize, amt: usize) -> usize {
        if self.ref_cnt_addr + pe * std::mem::size_of::<AtomicUsize>() < 10 {
            println!("error!!!! addrress makes no sense: {:?} ", pe);
            println!("{:?}", self);
            panic!();
        }
        let team_pe = pe;
        let tot_ref_cnt = unsafe {
            ((self.total_ref_cnt_addr + team_pe * std::mem::size_of::<AtomicUsize>())
                as *mut AtomicUsize)
                .as_ref()
                .expect("invalid darc addr")
        };
        tot_ref_cnt.fetch_add(amt, Ordering::SeqCst);
        let ref_cnt = unsafe {
            ((self.ref_cnt_addr + team_pe * std::mem::size_of::<AtomicUsize>()) as *mut AtomicUsize)
                .as_ref()
                .expect("invalid darc addr")
        };
        ref_cnt.fetch_add(amt, Ordering::SeqCst)
    }

    // async fn barrier(&self) -> &B{
    //     let barrier_fut = unsafe { (*self.barrier).async_barrier() };
    //     barrier_fut.await;
    // }

    fn update_item(&mut self, item: *const T) {
        self.item = item;
    }

    #[allow(dead_code)]
    fn item(&self) -> &T {
        unsafe { &(*self.item) }
    }

    fn send_finished(&self) -> Vec<AmHandle<()>> {
        let ref_cnts = unsafe {
            std::slice::from_raw_parts_mut(self.ref_cnt_addr as *mut AtomicUsize, self.num_pes)
        };
        let team = self.team();
        let mut reqs = vec![];
        for pe in 0..ref_cnts.len() {
            let cnt = ref_cnts[pe].swap(0, Ordering::SeqCst);

            if cnt > 0 {
                let my_addr = &*self as *const DarcInner<T> as usize;
                let pe_addr = team.lamellae.remote_addr(
                    team.arch.world_pe(pe).expect("invalid team member"),
                    my_addr,
                );
                // println!(
                //     "[{:?}] sending finished to {:?} {:?} team {:?} {:x}",
                //     std::thread::current().id(),
                //     pe,
                //     cnt,
                //     team.team_hash,
                //     my_addr
                // );
                // println!("[{:?}] {:?}", std::thread::current().id(), self);
                reqs.push(team.exec_am_pe_tg(
                    pe,
                    FinishedAm {
                        cnt: cnt,
                        src_pe: pe,
                        inner_addr: pe_addr,
                    },
                    Some(self.am_counters()),
                ));
            }
        }
        reqs
    }
    // unsafe fn any_ref_cnt(&self) -> bool {
    //     let ref_cnts =
    //         std::slice::from_raw_parts_mut(self.ref_cnt_addr as *mut usize, self.num_pes); //this is potentially a dirty read
    //     ref_cnts.iter().any(|x| *x > 0)
    // }

    // fn debug_print(&self) {
    //     let ref_cnts_slice = unsafe {
    //         std::slice::from_raw_parts_mut(self.ref_cnt_addr as *mut usize, self.num_pes)
    //     };
    //     let total_ref_cnts_slice = unsafe {
    //         std::slice::from_raw_parts_mut(self.total_ref_cnt_addr as *mut usize, self.num_pes)
    //     };
    //     println!(
    //         "[{:?}] refcnts: {:?} total_refcnts: {:?} lc: {:?} tlc: {:?} dc: {:?} tdc: {:?}",
    //         std::thread::current().id(),
    //         ref_cnts_slice,
    //         total_ref_cnts_slice,
    //         self.local_cnt.load(Ordering::SeqCst),
    //         self.total_local_cnt.load(Ordering::SeqCst),
    //         self.dist_cnt.load(Ordering::SeqCst),
    //         self.total_dist_cnt.load(Ordering::SeqCst)
    //     );
    // }

    async fn block_on_outstanding(inner: WrappedInner<T>, state: DarcMode, extra_cnt: usize) {
        let team = inner.team();
        let mode_refs =
            unsafe { std::slice::from_raw_parts_mut(inner.mode_addr as *mut u8, inner.num_pes) };
        if team.num_pes() == 1 {
            while inner.local_cnt.load(Ordering::SeqCst) > 1 + extra_cnt {
                async_std::task::yield_now().await;
            }
            unsafe {
                (*(((&mut mode_refs[inner.my_pe]) as *mut u8) as *mut AtomicU8)) //this should be fine given that DarcMode uses Repr(u8)
                    .store(state as u8, Ordering::SeqCst)
            };
        } else {
            let mut outstanding_refs = true;

            let mut prev_ref_cnts = vec![0usize; inner.num_pes];
            let mut barrier_id = 1usize;

            let barrier_ref_cnt_slice = unsafe {
                std::slice::from_raw_parts_mut(inner.mode_ref_cnt_addr as *mut usize, inner.num_pes)
            };
            let barrier_slice = unsafe {
                std::slice::from_raw_parts_mut(inner.mode_barrier_addr as *mut usize, inner.num_pes)
            };

            let ref_cnts_slice = unsafe {
                std::slice::from_raw_parts_mut(
                    inner.total_ref_cnt_addr as *mut usize,
                    inner.num_pes,
                )
            };

            // let rel_addr = inner.inner.as_ptr() as *const _ as usize - team.lamellae.base_addr();

            while inner.local_cnt.load(Ordering::SeqCst) > 1 + extra_cnt {
                async_std::task::yield_now().await;
            }

            // println!(
            //     "[{:?}] entering initial block_on barrier()",
            //     std::thread::current().id()
            // );
            let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
            barrier_fut.await;
            // println!(
            //     "[{:?}] leaving initial block_on barrier()",
            //     std::thread::current().id()
            // );

            while outstanding_refs {
                outstanding_refs = false;
                // these hopefully all get set to non zero later otherwise we still need to wait
                for id in &mut *barrier_slice {
                    *id = 0;
                }
                let old_barrier_id = barrier_id; //we potentially will set barrier_id to 0 but want to maintiain the previously highest value
                while inner.local_cnt.load(Ordering::SeqCst) > 1 + extra_cnt {
                    async_std::task::yield_now().await;
                }
                join_all(inner.send_finished()).await;
                let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
                barrier_fut.await;

                let mut old_ref_cnts = ref_cnts_slice.to_vec();
                let old_local_cnt = inner.total_local_cnt.load(Ordering::SeqCst);
                let old_dist_cnt = inner.total_dist_cnt.load(Ordering::SeqCst);

                let rdma = &team.lamellae;
                // let mut dist_cnts_changed = false;
                for pe in 0..inner.num_pes {
                    let ref_cnt_u8 = unsafe {
                        std::slice::from_raw_parts_mut(
                            &mut old_ref_cnts[pe] as *mut usize as *mut u8,
                            std::mem::size_of::<usize>(),
                        )
                    };
                    if prev_ref_cnts[pe] != old_ref_cnts[pe] {
                        let send_pe = team.arch.single_iter(pe).next().unwrap();
                        // println!(
                        //     "[{:?}] {rel_addr:x} sending {:?} to pe {:?} at {:x} + {:?} ({:x}) ",
                        //     std::thread::current().id(),
                        //     old_ref_cnts[pe],
                        //     pe,
                        //     inner.mode_ref_cnt_addr,
                        //     inner.my_pe * std::mem::size_of::<usize>(),
                        //     inner.mode_ref_cnt_addr + inner.my_pe * std::mem::size_of::<usize>()
                        // );
                        // println!("darc block_on_outstanding put 1");
                        rdma.put(
                            send_pe,
                            ref_cnt_u8,
                            inner.mode_ref_cnt_addr + inner.my_pe * std::mem::size_of::<usize>(), //this is barrier_ref_cnt_slice
                        );
                        // dist_cnts_changed = true;
                        outstanding_refs = true;
                        barrier_id = 0;
                    }
                }
                rdma.flush();
                let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
                barrier_fut.await;
                outstanding_refs |= old_local_cnt != inner.total_local_cnt.load(Ordering::SeqCst);
                // if outstanding_refs {
                //     println!(
                //         "[{:?}] {rel_addr:x}  total local cnt changed",
                //         std::thread::current().id()
                //     );
                // }
                outstanding_refs |= old_dist_cnt != inner.total_dist_cnt.load(Ordering::SeqCst);
                // if outstanding_refs {
                //     println!(
                //         "[{:?}] {rel_addr:x}  total dist cnt changed",
                //         std::thread::current().id()
                //     );
                // }

                let mut barrier_sum = 0;
                for pe in 0..inner.num_pes {
                    outstanding_refs |= old_ref_cnts[pe] != ref_cnts_slice[pe];
                    // if outstanding_refs {
                    //     println!(
                    //         "[{:?}] {rel_addr:x}  refs changed for pe {pe}",
                    //         std::thread::current().id()
                    //     );
                    // }
                    // dist_cnts_changed |= old_ref_cnts[pe] != ref_cnts_slice[pe];
                    barrier_sum += barrier_ref_cnt_slice[pe];
                }
                outstanding_refs |= barrier_sum != old_dist_cnt;
                // if outstanding_refs {
                //     println!(
                //         "[{:?}] {rel_addr:x}  sum of cnts != dist ref cnt {:?} {:?}",
                //         std::thread::current().id(),
                //         barrier_ref_cnt_slice,
                //         old_ref_cnts
                //     );
                // }
                if outstanding_refs {
                    // println!("reseting barrier_id");
                    barrier_id = 0;
                }
                rdma.flush();
                let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
                barrier_fut.await;

                for pe in 0..inner.num_pes {
                    let send_pe = team.arch.single_iter(pe).next().unwrap();
                    // println!(
                    //     "[{:?}] {rel_addr:x} sending {barrier_id} ({barrier_id_slice:?}) to pe {pe} ",
                    //     std::thread::current().id(),
                    // );
                    let barrier_id_slice = unsafe {
                        std::slice::from_raw_parts_mut(
                            &mut barrier_id as *mut usize as *mut u8,
                            std::mem::size_of::<usize>(),
                        )
                    };
                    // println!("darc block_on_outstanding put 2");
                    rdma.put(
                        send_pe,
                        barrier_id_slice,
                        inner.mode_barrier_addr + inner.my_pe * std::mem::size_of::<usize>(),
                    );
                }
                //maybe we need to change the above to a get?
                rdma.flush();
                let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
                barrier_fut.await;
                for id in &*barrier_slice {
                    outstanding_refs |= *id == 0;
                }
                // if outstanding_refs {
                //     println!("[{:?}] {rel_addr:x}  not all pes ready mode_refs: {mode_refs:?} prev_ref_cnts: {prev_ref_cnts:?} barrier_id: {barrier_id:?} barrier_id_slice: {barrier_id_slice:?} barrier_ref_cnt_slice: {barrier_ref_cnt_slice:?}
                //     barrier_slice: {barrier_slice:?} ref_cnts_slice: {ref_cnts_slice:?} old_ref_cnts: {old_ref_cnts:?} old_local_cnt: {old_local_cnt:?} local_cnt: {:?} old_dist_cnt: {old_dist_cnt:?} dist_cnt: {:?}
                //     dist_cnts_changed: {dist_cnts_changed:?} barrier_sum: {barrier_sum:?} old_barrier_id: {old_barrier_id:?} ", std::thread::current().id(),inner.total_local_cnt.load(Ordering::SeqCst), inner.total_dist_cnt.load(Ordering::SeqCst));
                // }
                // if dist_cnts_changed || !outstanding_refs {
                //     println!("[{:?}] {rel_addr:x}  mode_refs: {mode_refs:?} prev_ref_cnts: {prev_ref_cnts:?} barrier_id: {barrier_id:?} barrier_id_slice: {barrier_id_slice:?} barrier_ref_cnt_slice: {barrier_ref_cnt_slice:?}
                //     barrier_slice: {barrier_slice:?} ref_cnts_slice: {ref_cnts_slice:?} old_ref_cnts: {old_ref_cnts:?} old_local_cnt: {old_local_cnt:?} local_cnt: {:?} old_dist_cnt: {old_dist_cnt:?} dist_cnt: {:?}
                //     dist_cnts_changed: {dist_cnts_changed:?} barrier_sum: {barrier_sum:?} old_barrier_id: {old_barrier_id:?} ", std::thread::current().id(), inner.total_local_cnt.load(Ordering::SeqCst), inner.total_dist_cnt.load(Ordering::SeqCst));
                // }
                barrier_id = old_barrier_id + 1;
                // if outstanding_refs {
                //     // println!(
                //     //     "[{:?}] still outstanding, exec a task!",
                //     //     std::thread::current().id()
                //     // );
                //     // team.scheduler.exec_task();
                //     async_std::task::yield_now().await;
                // }
                prev_ref_cnts = old_ref_cnts;
                let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
                barrier_fut.await;
            }
            // println!(
            //     "[{:?}] {rel_addr:x}  all outstanding refs are resolved",
            //     std::thread::current().id()
            // );
            // inner.debug_print();
            // println!("[{:?}] {:?}", std::thread::current().id(), inner);

            unsafe {
                (*(((&mut mode_refs[inner.my_pe]) as *mut u8) as *mut AtomicU8)) //this should be fine given that DarcMode uses Repr(u8)
                    .store(state as u8, Ordering::SeqCst)
            };
            let rdma = &team.lamellae;
            for pe in team.arch.team_iter() {
                // println!("darc block_on_outstanding put 3");
                rdma.put(
                    pe,
                    &mode_refs[inner.my_pe..=inner.my_pe],
                    inner.mode_addr + inner.my_pe * std::mem::size_of::<DarcMode>(),
                );
            }
            for pe in mode_refs.iter() {
                let mut timer = std::time::Instant::now();
                while *pe != state as u8 {
                    if inner.local_cnt.load(Ordering::SeqCst) == 1 + extra_cnt {
                        join_all(inner.send_finished()).await;
                    }
                    if timer.elapsed().as_secs_f64() > config().deadlock_timeout {
                        let ref_cnts_slice = unsafe {
                            std::slice::from_raw_parts_mut(
                                inner.ref_cnt_addr as *mut usize,
                                inner.num_pes,
                            )
                        };
                        println!("[{:?}][{:?}][WARNING] -- Potential deadlock detected.\n\
                            The runtime is currently waiting for all remaining references to this distributed object to be dropped.\n\
                            The object is likely a {:?} with {:?} remaining local references and {:?} remaining remote references, ref cnts by pe {ref_cnts_slice:?}\n\
                            An example where this can occur can be found at https://docs.rs/lamellar/latest/lamellar/array/struct.ReadOnlyArray.html#method.into_local_lock\n\
                            The deadlock timeout can be set via the LAMELLAR_DEADLOCK_TIMEOUT environment variable, the current timeout is {} seconds\n\
                            To view backtrace set RUST_LIB_BACKTRACE=1\n\
                            {}",
                            inner.my_pe,
                            std::thread::current().id(),
                            unsafe {
                                &std::slice::from_raw_parts_mut(inner.mode_addr as *mut DarcMode, inner.num_pes)
                            },
                            inner.local_cnt.load(Ordering::SeqCst),
                            inner.dist_cnt.load(Ordering::SeqCst),
                            config().deadlock_timeout,
                            std::backtrace::Backtrace::capture()
                        );
                        timer = std::time::Instant::now();
                    }
                    async_std::task::yield_now().await;
                }
            }

            // self.debug_print();
            // println!("{rel_addr:x}  {:?}", self);
            let barrier_fut = unsafe { inner.barrier.as_ref().unwrap().async_barrier() };
            barrier_fut.await;
        }

        // self.debug_print();
    }

    // fn wait_all(&self) {
    //     let mut temp_now = Instant::now();
    //     // let mut first = true;
    //     let team = self.team();
    //     // team.flush();
    //     let am_counters = self.am_counters();
    //     while am_counters.outstanding_reqs.load(Ordering::SeqCst) > 0 {
    //         // std::thread::yield_now();
    //         team.scheduler.exec_task(); //mmight as well do useful work while we wait
    //         if temp_now.elapsed().as_secs_f64() > config().deadlock_timeout {
    //             //|| first{
    //             // println!(
    //             //     "[{:?}] in darc wait_all mype: {:?} cnt: {:?} {:?}",
    //             //     std::thread::current().id(),
    //             //     team.world_pe,
    //             //     am_counters.send_req_cnt.load(Ordering::SeqCst),
    //             //     am_counters.outstanding_reqs.load(Ordering::SeqCst),
    //             // );
    //             temp_now = Instant::now();
    //             // first = false;
    //         }
    //     }
    //     // println!("done in wait all {:?}",std::time::SystemTime::now());
    // }
}

impl<T> fmt::Debug for DarcInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:}/{:?}] lc: {:?} dc: {:?} wc: {:?}\nref_cnt: {:?}\n am_cnt ({:?},{:?})\nmode {:?}",
            self.my_pe,
            self.num_pes,
            self.local_cnt.load(Ordering::SeqCst),
            self.dist_cnt.load(Ordering::SeqCst),
            self.weak_local_cnt.load(Ordering::SeqCst),
            unsafe {
                &std::slice::from_raw_parts_mut(self.ref_cnt_addr as *mut usize, self.num_pes)
            },
            self.am_counters().outstanding_reqs.load(Ordering::Relaxed),
            self.am_counters().send_req_cnt.load(Ordering::Relaxed),
            unsafe {
                &std::slice::from_raw_parts_mut(self.mode_addr as *mut DarcMode, self.num_pes)
            }
        )
    }
}

impl<T> Darc<T> {
    #[doc(hidden)]
    pub fn downgrade(the_darc: &Darc<T>) -> WeakDarc<T> {
        // println!("downgrading darc ");
        // the_darc.print();
        the_darc
            .inner()
            .weak_local_cnt
            .fetch_add(1, Ordering::SeqCst);
        let weak = WeakDarc {
            inner: the_darc.inner,
            src_pe: the_darc.src_pe,
        };
        // the_darc.print();
        weak
    }
    fn inner(&self) -> &DarcInner<T> {
        unsafe { self.inner.as_ref().expect("invalid darc inner ptr") }
    }
    fn inner_mut(&self) -> &mut DarcInner<T> {
        unsafe { self.inner.as_mut().expect("invalid darc inner ptr") }
    }
    #[allow(dead_code)]
    pub(crate) fn team(&self) -> Pin<Arc<LamellarTeamRT>> {
        self.inner().team()
    }
    fn ref_cnts_as_mut_slice(&self) -> &mut [usize] {
        let inner = self.inner();
        unsafe { std::slice::from_raw_parts_mut(inner.ref_cnt_addr as *mut usize, inner.num_pes) }
    }
    fn mode_as_mut_slice(&self) -> &mut [DarcMode] {
        let inner = self.inner();
        unsafe { std::slice::from_raw_parts_mut(inner.mode_addr as *mut DarcMode, inner.num_pes) }
    }
    fn mode_barrier_as_mut_slice(&self) -> &mut [usize] {
        let inner = self.inner();
        unsafe {
            std::slice::from_raw_parts_mut(inner.mode_barrier_addr as *mut usize, inner.num_pes)
        }
    }
    fn mode_ref_cnt_as_mut_slice(&self) -> &mut [usize] {
        let inner = self.inner();
        unsafe {
            std::slice::from_raw_parts_mut(inner.mode_ref_cnt_addr as *mut usize, inner.num_pes)
        }
    }

    #[doc(hidden)]
    pub fn serialize_update_cnts(&self, cnt: usize) {
        // println!("serialize darc cnts");
        self.inner()
            .dist_cnt
            .fetch_add(cnt, std::sync::atomic::Ordering::SeqCst);
        self.inner()
            .total_dist_cnt
            .fetch_add(cnt, std::sync::atomic::Ordering::SeqCst);
        // self.print();
        // println!("done serialize darc cnts");
    }

    #[doc(hidden)]
    pub fn deserialize_update_cnts(&self) {
        // println!("deserialize darc? cnts");
        self.inner().inc_pe_ref_count(self.src_pe, 1);
        self.inner().local_cnt.fetch_add(1, Ordering::SeqCst);
        self.inner().total_local_cnt.fetch_add(1, Ordering::SeqCst);
        // println! {"[{:?}] darc[{:?}] deserialized {:?} {:?} {:?}",std::thread::current().id(),self.inner().id,self.inner,self.inner().local_cnt.load(Ordering::SeqCst), self.inner().total_local_cnt.load(Ordering::SeqCst)};
        // self.print();
        // println!("done deserialize darc cnts");
    }

    #[doc(hidden)]
    pub fn inc_local_cnt(&self, cnt: usize) {
        self.inner().local_cnt.fetch_add(cnt, Ordering::SeqCst);
        self.inner()
            .total_local_cnt
            .fetch_add(cnt, Ordering::SeqCst);
        // println!(
        //     "[{:?}] darc[{:?}] inc_local_cnt {:?} {:?}",
        //     std::thread::current().id(),
        //     self.inner().id,
        //     self.inner().local_cnt.load(Ordering::SeqCst),
        //     self.inner().total_local_cnt.load(Ordering::SeqCst)
        // );
    }

    #[doc(hidden)]
    pub fn print(&self) {
        let rel_addr = unsafe { self.inner as usize - (*self.inner().team).lamellae.base_addr() };
        println!(
            "[{:?}]--------\nid: {:?} orig: {:?} ({:?} (0x{:x}) item_addr {:?} {:?}\n--------[{:?}]",
            std::thread::current().id(),
            self.inner().id,
            self.src_pe,
            self.inner,
            rel_addr,
            self.inner().item,
            self.inner(),
            std::thread::current().id(),
        );
    }
}

impl<T> Darc<T> {
    #[doc(alias = "Collective")]
    /// Constructs a new `Darc<T>` on the PEs specified by team.
    ///
    /// This is a blocking collective call amongst all PEs in the team, only returning once every PE in the team has completed the call.
    ///
    /// Returns an error if this PE is not a part of team
    ///
    /// # Collective Operation
    /// Requires all PEs associated with the `team` to enter the constructor call otherwise deadlock will occur (i.e. team barriers are being called internally)
    ///
    /// # Examples
    ///
    /// ```
    /// use lamellar::darc::prelude::*;
    ///
    /// let world = LamellarWorldBuilder::new().build();
    ///
    /// let five = Darc::new(&world,5).expect("PE in world team");
    /// ```
    pub fn new<U: Into<IntoLamellarTeam>>(team: U, item: T) -> Result<Darc<T>, IdError> {
        Darc::try_new_with_drop(team, item, DarcMode::Darc, None)
    }

    pub(crate) async fn async_try_new<U: Into<IntoLamellarTeam>>(
        team: U,
        item: T,
        state: DarcMode,
    ) -> Result<Darc<T>, IdError> {
        Darc::async_try_new_with_drop(team, item, state, None).await
    }

    pub(crate) fn try_new<U: Into<IntoLamellarTeam>>(
        team: U,
        item: T,
        state: DarcMode,
    ) -> Result<Darc<T>, IdError> {
        Darc::try_new_with_drop(team, item, state, None)
    }

    pub(crate) async fn async_try_new_with_drop<U: Into<IntoLamellarTeam>>(
        team: U,
        item: T,
        state: DarcMode,
        drop: Option<fn(&mut T)>,
    ) -> Result<Darc<T>, IdError> {
        let team_rt = team.into().team.clone();
        let my_pe = team_rt.team_pe?;

        let alloc = if team_rt.num_pes == team_rt.num_world_pes {
            AllocationType::Global
        } else {
            AllocationType::Sub(team_rt.get_pes())
        };

        let size = std::mem::size_of::<DarcInner<T>>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<DarcMode>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<usize>();
        // println!("creating new darc");

        team_rt.async_barrier().await;
        // println!("creating new darc after barrier");
        let addr = team_rt
            .lamellae
            .alloc(size, alloc, std::mem::align_of::<DarcInner<T>>())
            .expect("out of memory");
        // let temp_team = team_rt.clone();
        // team_rt.print_cnt();
        let team_ptr = unsafe {
            let pinned_team = Pin::into_inner_unchecked(team_rt.clone());
            Arc::into_raw(pinned_team)
        };
        // team_rt.print_cnt();
        let am_counters = Arc::new(AMCounters::new());
        let am_counters_ptr = Arc::into_raw(am_counters);
        let barrier = Box::new(Barrier::new(
            team_rt.world_pe,
            team_rt.num_world_pes,
            team_rt.lamellae.clone(),
            team_rt.arch.clone(),
            team_rt.scheduler.clone(),
            team_rt.panic.clone(),
        ));
        let barrier_ptr = Box::into_raw(barrier);
        let darc_temp = DarcInner {
            id: DARC_ID.fetch_add(1, Ordering::Relaxed),
            my_pe: my_pe,
            num_pes: team_rt.num_pes,
            local_cnt: AtomicUsize::new(1),
            total_local_cnt: AtomicUsize::new(1),
            weak_local_cnt: AtomicUsize::new(0),
            dist_cnt: AtomicUsize::new(0),
            total_dist_cnt: AtomicUsize::new(0),
            ref_cnt_addr: addr + std::mem::size_of::<DarcInner<T>>(),
            total_ref_cnt_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            mode_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            mode_ref_cnt_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<DarcMode>(),
            mode_barrier_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<DarcMode>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            barrier: barrier_ptr,
            // mode_barrier_rounds: num_rounds,
            am_counters: am_counters_ptr,
            team: team_ptr, //&team_rt, //Arc::into_raw(temp_team),
            item: Box::into_raw(Box::new(item)),
            drop: drop,
            valid: AtomicBool::new(true),
        };
        unsafe {
            std::ptr::copy_nonoverlapping(&darc_temp, addr as *mut DarcInner<T>, 1);
        }
        // println!("Darc Inner Item Addr: {:?}", darc_temp.item);

        let d = Darc {
            inner: addr as *mut DarcInner<T>,
            src_pe: my_pe,
        };
        for elem in d.ref_cnts_as_mut_slice() {
            *elem = 0;
        }
        for elem in d.mode_as_mut_slice() {
            *elem = state;
        }
        for elem in d.mode_barrier_as_mut_slice() {
            *elem = 0;
        }
        for elem in d.mode_ref_cnt_as_mut_slice() {
            *elem = 0;
        }
        // println!(
        //     " [{:?}] created new darc , next_id: {:?}",
        //     std::thread::current().id(),
        //     DARC_ID.load(Ordering::Relaxed)
        // );
        // d.print();
        team_rt.async_barrier().await;
        // team_rt.print_cnt();
        Ok(d)
    }

    pub(crate) fn try_new_with_drop<U: Into<IntoLamellarTeam>>(
        team: U,
        item: T,
        state: DarcMode,
        drop: Option<fn(&mut T)>,
    ) -> Result<Darc<T>, IdError> {
        let team_rt = team.into().team.clone();
        let my_pe = team_rt.team_pe?;

        let alloc = if team_rt.num_pes == team_rt.num_world_pes {
            AllocationType::Global
        } else {
            AllocationType::Sub(team_rt.get_pes())
        };

        let size = std::mem::size_of::<DarcInner<T>>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<DarcMode>()
            + team_rt.num_pes * std::mem::size_of::<usize>()
            + team_rt.num_pes * std::mem::size_of::<usize>();
        // println!("creating new darc");

        team_rt.tasking_barrier();
        // println!("creating new darc after barrier");
        let addr = team_rt
            .lamellae
            .alloc(size, alloc, std::mem::align_of::<DarcInner<T>>())
            .expect("out of memory");
        // let temp_team = team_rt.clone();
        // team_rt.print_cnt();
        let team_ptr = unsafe {
            let pinned_team = Pin::into_inner_unchecked(team_rt.clone());
            Arc::into_raw(pinned_team)
        };
        // team_rt.print_cnt();
        let am_counters = Arc::new(AMCounters::new());
        let am_counters_ptr = Arc::into_raw(am_counters);
        let barrier = Box::new(Barrier::new(
            team_rt.world_pe,
            team_rt.num_world_pes,
            team_rt.lamellae.clone(),
            team_rt.arch.clone(),
            team_rt.scheduler.clone(),
            team_rt.panic.clone(),
        ));
        let barrier_ptr = Box::into_raw(barrier);
        let darc_temp = DarcInner {
            id: DARC_ID.fetch_add(1, Ordering::Relaxed),
            my_pe: my_pe,
            num_pes: team_rt.num_pes,
            local_cnt: AtomicUsize::new(1),
            total_local_cnt: AtomicUsize::new(1),
            weak_local_cnt: AtomicUsize::new(0),
            dist_cnt: AtomicUsize::new(0),
            total_dist_cnt: AtomicUsize::new(0),
            ref_cnt_addr: addr + std::mem::size_of::<DarcInner<T>>(),
            total_ref_cnt_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            mode_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            mode_ref_cnt_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<DarcMode>(),
            mode_barrier_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<usize>()
                + team_rt.num_pes * std::mem::size_of::<DarcMode>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            barrier: barrier_ptr,
            // mode_barrier_rounds: num_rounds,
            am_counters: am_counters_ptr,
            team: team_ptr, //&team_rt, //Arc::into_raw(temp_team),
            item: Box::into_raw(Box::new(item)),
            drop: drop,
            valid: AtomicBool::new(true),
        };
        unsafe {
            std::ptr::copy_nonoverlapping(&darc_temp, addr as *mut DarcInner<T>, 1);
        }
        // println!("Darc Inner Item Addr: {:?}", darc_temp.item);

        let d = Darc {
            inner: addr as *mut DarcInner<T>,
            src_pe: my_pe,
        };
        for elem in d.ref_cnts_as_mut_slice() {
            *elem = 0;
        }
        for elem in d.mode_as_mut_slice() {
            *elem = state;
        }
        for elem in d.mode_barrier_as_mut_slice() {
            *elem = 0;
        }
        for elem in d.mode_ref_cnt_as_mut_slice() {
            *elem = 0;
        }
        // println!(
        //     " [{:?}] created new darc , next_id: {:?}",
        //     std::thread::current().id(),
        //     DARC_ID.load(Ordering::Relaxed)
        // );
        // d.print();
        team_rt.tasking_barrier();
        // team_rt.print_cnt();
        Ok(d)
    }

    pub(crate) async fn block_on_outstanding(self, state: DarcMode, extra_cnt: usize) {
        let wrapped = WrappedInner {
            inner: NonNull::new(self.inner as *mut DarcInner<T>).expect("invalid darc pointer"),
        };
        DarcInner::block_on_outstanding(wrapped, state, extra_cnt).await;
    }

    #[doc(alias = "Collective")]
    /// Converts this Darc into a [LocalRwDarc]
    ///
    /// This is a blocking collective call amongst all PEs in the Darc's team, only returning once every PE in the team has completed the call.
    ///
    /// Furthermore, this call will block while any additional references outside of the one making this call exist on each PE. It is not possible for the
    /// pointed to object to wrapped by both a Darc and a LocalRwDarc simultaneously (on any PE).
    ///
    /// # Collective Operation
    /// Requires all PEs associated with the `darc` to enter the call otherwise deadlock will occur (i.e. team barriers are being called internally)
    ///
    /// # Examples
    /// ```
    /// use lamellar::darc::prelude::*;
    ///
    /// let world = LamellarWorldBuilder::new().build();
    ///
    /// let five = Darc::new(&world,5).expect("PE in world team");
    /// let five_as_localdarc = five.into_localrw();
    /// ```
    pub fn into_localrw(self) -> LocalRwDarc<T> {
        let inner = self.inner();
        let _cur_pe = inner.team().world_pe;
        inner.team().block_on(DarcInner::block_on_outstanding(
            WrappedInner {
                inner: NonNull::new(self.inner as *mut DarcInner<T>).expect("invalid darc pointer"),
            },
            DarcMode::LocalRw,
            0,
        ));
        inner.local_cnt.fetch_add(1, Ordering::SeqCst); //we add this here because to account for moving inner into d
        inner.total_local_cnt.fetch_add(1, Ordering::SeqCst);
        // println! {"[{:?}] darc[{:?}] into_localrw {:?} {:?} {:?}",std::thread::current().id(),self.inner().id,self.inner,self.inner().local_cnt.load(Ordering::SeqCst),self.inner().total_local_cnt.load(Ordering::SeqCst)};
        let item = unsafe { *Box::from_raw(inner.item as *mut T) };

        let d = Darc {
            inner: self.inner as *mut DarcInner<Arc<RwLock<T>>>,
            src_pe: self.src_pe,
        };
        d.inner_mut()
            .update_item(Box::into_raw(Box::new(Arc::new(RwLock::new(item)))));
        // d.print();
        LocalRwDarc { darc: d }
    }

    #[doc(alias = "Collective")]
    /// Converts this Darc into a [GlobalRwDarc]
    ///
    /// This is a blocking collective call amongst all PEs in the Darc's team, only returning once every PE in the team has completed the call.
    ///
    /// Furthermore, this call will block while any additional references outside of the one making this call exist on each PE. It is not possible for the
    /// pointed to object to wrapped by both a GlobalRwDarc and a Darc simultaneously (on any PE).
    ///
    /// # Collective Operation
    /// Requires all PEs associated with the `darc` to enter the call otherwise deadlock will occur (i.e. team barriers are being called internally)
    ///
    /// # Examples
    /// ```
    /// use lamellar::darc::prelude::*;
    ///
    /// let world = LamellarWorldBuilder::new().build();
    ///
    /// let five = Darc::new(&world,5).expect("PE in world team");
    /// let five_as_globaldarc = five.into_globalrw();
    /// ```
    pub fn into_globalrw(self) -> GlobalRwDarc<T> {
        let inner = self.inner();
        let _cur_pe = inner.team().world_pe;
        inner.team().block_on(DarcInner::block_on_outstanding(
            WrappedInner {
                inner: NonNull::new(self.inner as *mut DarcInner<T>).expect("invalid darc pointer"),
            },
            DarcMode::GlobalRw,
            0,
        ));
        inner.local_cnt.fetch_add(1, Ordering::SeqCst); //we add this here because to account for moving inner into d
        inner.total_local_cnt.fetch_add(1, Ordering::SeqCst);
        // println! {"[{:?}] darc[{:?}] into_globalrw {:?} {:?} {:?}",std::thread::current().id(),self.inner().id,self.inner,self.inner().local_cnt.load(Ordering::SeqCst),self.inner().total_local_cnt.load(Ordering::SeqCst)};

        let item = unsafe { Box::from_raw(inner.item as *mut T) };
        let d = Darc {
            inner: self.inner as *mut DarcInner<DistRwLock<T>>,
            src_pe: self.src_pe,
        };
        d.inner_mut()
            .update_item(Box::into_raw(Box::new(DistRwLock::new(
                *item,
                self.inner().team(),
            ))));
        GlobalRwDarc { darc: d }
    }
}

impl<T> Clone for Darc<T> {
    fn clone(&self) -> Self {
        self.inner().local_cnt.fetch_add(1, Ordering::SeqCst);
        self.inner().total_local_cnt.fetch_add(1, Ordering::SeqCst);
        // println! {"[{:?}] darc[{:?}] cloned {:?} {:?} {:?}", std::thread::current().id(),self.inner().id,self.inner,self.inner().local_cnt.load(Ordering::SeqCst),self.inner().total_local_cnt.load(Ordering::SeqCst)};
        // self.print();
        Darc {
            inner: self.inner,
            src_pe: self.src_pe,
        }
    }
}

impl<T> Deref for Darc<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // println!(
        //     "[{:?}] deref called my address {:?}",
        //     std::thread::current().id(),
        //     self as *const _
        // );
        // println!(
        //     "[{:?}] deref called inner address: {:?}",
        //     std::thread::current().id(),
        //     self.inner
        // );
        // println!(
        //     "[{:?}] deref called item address: {:?}",
        //     std::thread::current().id(),
        //     self.inner().item
        // );
        // unsafe { &*self.inner().item }
        self.inner().item()
    }
}

impl<T: fmt::Display> fmt::Display for Darc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug> fmt::Debug for Darc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

macro_rules! local_mode {
    ($mode:expr,$mode_refs:ident,$inner:ident) => {{
        let local_mode = unsafe {
            (*(((&mut $mode_refs[$inner.my_pe]) as *mut DarcMode) as *mut AtomicU8))
                .compare_exchange(
                    $mode as u8,
                    DarcMode::Dropped as u8,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
        };
        local_mode == Ok($mode as u8)
    }};
}

macro_rules! launch_drop {
    ($mode:ty, $inner:ident, $inner_addr:expr) => {
        // println!("launching drop task as {}", stringify!($mode));
        let team = $inner.team();
        // team.print_cnt();
        team.exec_am_local(DroppedWaitAM {
            inner_addr: $inner_addr as *const u8 as usize,
            mode_addr: $inner.mode_addr,
            my_pe: $inner.my_pe,
            num_pes: $inner.num_pes,
            team: team.clone(),
            phantom: PhantomData::<T>,
        });
    };
}

impl<T: 'static> Drop for Darc<T> {
    fn drop(&mut self) {
        let inner = self.inner();
        let cnt = inner.local_cnt.fetch_sub(1, Ordering::SeqCst);
        // println! {"[{:?}] darc[{:?}]  dropped {:?} {:?} {:?}",std::thread::current().id(),self.inner().id,self.inner,self.inner().local_cnt.load(Ordering::SeqCst),inner.total_local_cnt.load( Ordering::SeqCst)};
        // self.print();
        if cnt == 1 {
            //we are currently the last local ref, if it increases again it must mean someone else has come in and we can probably let them worry about cleaning up...
            let pe_ref_cnts = self.ref_cnts_as_mut_slice();
            // println!(
            //     "[{:?}] Last local ref... for now! {:?}",
            //     std::thread::current().id(),
            //     pe_ref_cnts
            // );
            // self.print();
            if pe_ref_cnts.iter().any(|&x| x > 0) {
                //if we have received and accesses from remote pes, send we are finished
                inner.send_finished();
            }
        }
        // println!("in drop");
        // self.print();
        if inner.local_cnt.load(Ordering::SeqCst) == 0 {
            // we have no more current local references so lets try to launch our garbage collecting am

            // println!("[{:?}] launching drop task", std::thread::current().id());

            let mode_refs = self.mode_as_mut_slice();
            if local_mode!(DarcMode::Darc, mode_refs, inner) {
                launch_drop!(DarcMode::Darc, inner, self.inner);
            } else if local_mode!(DarcMode::LocalRw, mode_refs, inner) {
                launch_drop!(DarcMode::LocalRw, inner, self.inner);
            } else if local_mode!(DarcMode::GlobalRw, mode_refs, inner) {
                launch_drop!(DarcMode::GlobalRw, inner, self.inner);
            } else if local_mode!(DarcMode::LocalRw, mode_refs, inner) {
                launch_drop!(DarcMode::LocalRw, inner, self.inner);
            } else if local_mode!(DarcMode::UnsafeArray, mode_refs, inner) {
                launch_drop!(DarcMode::UnsafeArray, inner, self.inner);
            } else if local_mode!(DarcMode::ReadOnlyArray, mode_refs, inner) {
                launch_drop!(DarcMode::ReadOnlyArray, inner, self.inner);
            }
            // else if local_mode!(DarcMode::LocalOnlyArray, mode_refs, inner) {
            //     launch_drop!(DarcMode::LocalOnlyArray, inner, self.inner);
            // }
            else if local_mode!(DarcMode::LocalLockArray, mode_refs, inner) {
                launch_drop!(DarcMode::LocalLockArray, inner, self.inner);
            } else if local_mode!(DarcMode::GlobalLockArray, mode_refs, inner) {
                launch_drop!(DarcMode::GlobalLockArray, inner, self.inner);
            } else if local_mode!(DarcMode::GenericAtomicArray, mode_refs, inner) {
                launch_drop!(DarcMode::GenericAtomicArray, inner, self.inner);
            } else if local_mode!(DarcMode::NativeAtomicArray, mode_refs, inner) {
                launch_drop!(DarcMode::NativeAtomicArray, inner, self.inner);
            }
            // self.print();
        }
        // self.print();
    }
}

#[lamellar_impl::AmLocalDataRT]
struct DroppedWaitAM<T> {
    inner_addr: usize,
    mode_addr: usize,
    my_pe: usize,
    num_pes: usize,
    team: Pin<Arc<LamellarTeamRT>>, //we include this to insure the team isnt dropped until the darc has been fully dropped across the system.
    phantom: PhantomData<T>,
}

impl<T> std::fmt::Debug for DroppedWaitAM<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DroppedWaitAM {{ inner_addr: {:?}, mode_addr: {:?}, my_pe: {:?}, num_pes: {:?}, team: {:?} }}", self.inner_addr, self.mode_addr, self.my_pe, self.num_pes, self.team)
    }
}

unsafe impl<T> Send for DroppedWaitAM<T> {}
unsafe impl<T> Sync for DroppedWaitAM<T> {}

pub(crate) struct WrappedInner<T> {
    inner: NonNull<DarcInner<T>>,
}
unsafe impl<T> Send for WrappedInner<T> {}

impl<T> Clone for WrappedInner<T> {
    fn clone(&self) -> Self {
        WrappedInner { inner: self.inner }
    }
}

impl<T> std::fmt::Debug for WrappedInner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WrappedInner {{ inner: {:?} }}", unsafe {
            self.inner.as_ref()
        })
    }
}

impl<T> std::ops::Deref for WrappedInner<T> {
    type Target = DarcInner<T>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.as_ref() }
    }
}

#[lamellar_impl::rt_am_local]
impl<T: 'static> LamellarAM for DroppedWaitAM<T> {
    async fn exec(self) {
        let mode_refs = unsafe {
            std::slice::from_raw_parts_mut(self.mode_addr as *mut DarcMode, self.num_pes)
        };

        let mut timeout = std::time::Instant::now();
        let wrapped = WrappedInner {
            inner: NonNull::new(self.inner_addr as *mut DarcInner<T>)
                .expect("invalid darc pointer"),
        };

        // println!(
        //     "[{:?}] in DroppedWaitAM {:x} {:?} {:?}",
        //     std::thread::current().id(),
        //     self.inner_addr,
        //     wrapped.id,
        //     wrapped.total_local_cnt.fetch_add(1, Ordering::SeqCst)
        // );
        let block_on_fut =
            { DarcInner::block_on_outstanding(wrapped.clone(), DarcMode::Dropped, 0) };
        block_on_fut.await;

        // println!(
        //     "[{:?}] past block_on_outstanding {:x}",
        //     std::thread::current().id(),
        //     self.inner_addr
        // );
        for pe in mode_refs.iter() {
            while *pe != DarcMode::Dropped {
                async_std::task::yield_now().await;

                if wrapped.local_cnt.load(Ordering::SeqCst) == 0 {
                    join_all(wrapped.send_finished()).await;
                }

                if timeout.elapsed().as_secs_f64() > config().deadlock_timeout {
                    let ref_cnts_slice = unsafe {
                        std::slice::from_raw_parts_mut(
                            wrapped.ref_cnt_addr as *mut usize,
                            wrapped.num_pes,
                        )
                    };

                    println!("[{:?}][WARNING] -- Potential deadlock detected when trying to free distributed object.\n\
                        The runtime is currently waiting for all remaining references to this distributed object to be dropped.\n\
                        The current status of the object on each pe is {:?} with {:?} remaining local references and {:?} remaining remote references, ref cnts by pe {ref_cnts_slice:?}\n\
                        the deadlock timeout can be set via the LAMELLAR_DEADLOCK_TIMEOUT environment variable, the current timeout is {} seconds\n\
                        To view backtrace set RUST_LIB_BACKTRACE=1\n\
                        {}",
                        std::thread::current().id(),
                        mode_refs,
                        wrapped.local_cnt.load(Ordering::SeqCst),
                        wrapped.dist_cnt.load(Ordering::SeqCst),
                        config().deadlock_timeout,
                        std::backtrace::Backtrace::capture()
                    );
                    timeout = std::time::Instant::now();
                }
            }
        }
        // let inner =self.inner_addr as *mut DarcInner<T>;
        let wrapped = WrappedInner {
            inner: NonNull::new(self.inner_addr as *mut DarcInner<T>)
                .expect("invalid darc pointer"),
        };

        // let inner = unsafe {&*wrapped.inner}; //we dont actually care about the "type" we wrap here, we just need access to the meta data for the darc (but still allow async wait cause T is not send)
        unsafe {
            wrapped.valid.store(false, Ordering::SeqCst);
            while wrapped.dist_cnt.load(Ordering::SeqCst) != 0
                || wrapped.local_cnt.load(Ordering::SeqCst) != 0
            {
                if wrapped.local_cnt.load(Ordering::SeqCst) == 0 {
                    // wrapped.send_finished()
                    join_all(wrapped.send_finished()).await;
                }
                if timeout.elapsed().as_secs_f64() > config().deadlock_timeout {
                    let ref_cnts_slice = std::slice::from_raw_parts_mut(
                        wrapped.ref_cnt_addr as *mut usize,
                        wrapped.num_pes,
                    );

                    println!("[{:?}][WARNING] --- Potential deadlock detected when trying to free distributed object.\n\
                        The runtime is currently waiting for all remaining references to this distributed object to be dropped.\n\
                        The current status of the object on each pe is {:?} with {:?} remaining local references and {:?} remaining remote references, ref cnts by pe {ref_cnts_slice:?}\n\
                        the deadlock timeout can be set via the LAMELLAR_DEADLOCK_TIMEOUT environment variable, the current timeout is {} seconds\n\
                        To view backtrace set RUST_LIB_BACKTRACE=1\n\
                        {}",
                        std::thread::current().id(),
                        mode_refs,
                        wrapped.local_cnt.load(Ordering::SeqCst),
                        wrapped.dist_cnt.load(Ordering::SeqCst),
                        config().deadlock_timeout,
                        std::backtrace::Backtrace::capture()
                    );
                    timeout = std::time::Instant::now();
                }
                async_std::task::yield_now().await;
            }
            {
                let mut _item = Box::from_raw(wrapped.item as *mut T);
                if let Some(my_drop) = wrapped.drop {
                    // println!("Dropping darc {:x}", self.inner_addr);
                    my_drop(&mut _item);
                } else {
                    // println!("no drop function for item {:x}", self.inner_addr);
                }
            }
            while wrapped.weak_local_cnt.load(Ordering::SeqCst) != 0 {
                //we can't actually free the darc memory until all weak pointers are gone too
                async_std::task::yield_now().await;
            }
            let _team = Arc::from_raw(wrapped.team); //return to rust to drop appropriately
                                                     // println!("team cnt: {:?}", Arc::strong_count(&_team));
                                                     // println!("Darc freed! {:x} {:?}",self.inner_addr,mode_refs);
            let _am_counters = Arc::from_raw(wrapped.am_counters);
            let _barrier = Box::from_raw(wrapped.barrier);
            self.team.lamellae.free(self.inner_addr);
            // println!(
            //     "[{:?}]leaving DroppedWaitAM {:x}",
            //     std::thread::current().id(),
            //     self.inner_addr
            // );
        }
    }
}

#[doc(hidden)]
#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct __NetworkDarc {
    inner_addr: usize,
    backend: Backend,
    orig_world_pe: usize,
    orig_team_pe: usize,
}

impl std::fmt::Debug for __NetworkDarc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NetworkDarc {{ inner_addr: {:x}, backend: {:?}, orig_world_pe: {:?}, orig_team_pe: {:?} }}", self.inner_addr, self.backend, self.orig_world_pe, self.orig_team_pe)
    }
}

impl<T> From<Darc<T>> for __NetworkDarc {
    fn from(darc: Darc<T>) -> Self {
        // println!("net darc from darc");
        let team = &darc.inner().team();
        let ndarc = __NetworkDarc {
            inner_addr: darc.inner as *const u8 as usize,
            backend: team.lamellae.backend(),
            orig_world_pe: team.world_pe,
            orig_team_pe: team.team_pe.expect("darcs only valid on team members"),
        };
        // darc.print();
        ndarc
    }
}

impl<T> From<&Darc<T>> for __NetworkDarc {
    fn from(darc: &Darc<T>) -> Self {
        // println!("net darc from darc");
        let team = &darc.inner().team();
        let ndarc = __NetworkDarc {
            inner_addr: darc.inner as *const u8 as usize,
            backend: team.lamellae.backend(),
            orig_world_pe: team.world_pe,
            orig_team_pe: team.team_pe.expect("darcs only valid on team members"),
        };
        // darc.print();
        ndarc
    }
}

impl<T> From<__NetworkDarc> for Darc<T> {
    fn from(ndarc: __NetworkDarc) -> Self {
        if let Some(lamellae) = LAMELLAES.read().get(&ndarc.backend) {
            let darc = Darc {
                inner: lamellae.local_addr(ndarc.orig_world_pe, ndarc.inner_addr)
                    as *mut DarcInner<T>,
                src_pe: ndarc.orig_team_pe,
            };
            darc
        } else {
            println!(
                "ndarc: 0x{:x} {:?} {:?} {:?} ",
                ndarc.inner_addr, ndarc.backend, ndarc.orig_world_pe, ndarc.orig_team_pe
            );
            panic!("unexepected lamellae backend {:?}", &ndarc.backend);
        }
    }
}
