use core::marker::PhantomData;
use futures::Future;
use parking_lot::RwLock;
use serde::{Deserialize, Deserializer};
use std::cmp::PartialEq;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::active_messaging::AMCounters;
use crate::lamellae::{AllocationType, Backend, LamellaeComm, LamellaeRDMA};
use crate::lamellar_team::{IntoLamellarTeam, LamellarTeamRT};
use crate::lamellar_world::LAMELLAES;
use crate::scheduler::SchedulerQueue;
use crate::IdError;

pub(crate) mod local_rw_darc;
use local_rw_darc::LocalRwDarc;

pub(crate) mod global_rw_darc;
use global_rw_darc::{DistRwLock, GlobalRwDarc};

#[repr(u8)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub(crate) enum DarcMode {
    Dropped,
    Darc,
    LocalRw,
    GlobalRw,
    UnsafeArray,
    ReadOnlyArray,
    LocalOnlyArray,
    // AtomicArray,
    GenericAtomicArray,
    NativeAtomicArray,
    LocalLockAtomicArray,
}

#[lamellar_impl::AmDataRT(Debug)]
struct FinishedAm {
    cnt: usize,
    src_pe: usize,
    inner_addr: usize, //cant pass the darc itself cause we cant handle generics yet in lamellarAM...
}

#[lamellar_impl::rt_am]
impl LamellarAM for FinishedAm {
    fn exec() {
        // println!("in finished! {:?}",self);
        let inner = unsafe { &*(self.inner_addr as *mut DarcInner<()>) }; //we dont actually care about the "type" we wrap here, we just need access to the meta data for the darc
        inner.dist_cnt.fetch_sub(self.cnt, Ordering::SeqCst);
    }
}

#[repr(C)]
pub struct DarcInner<T> {
    my_pe: usize,           // with respect to LamellarArch used to create this object
    num_pes: usize,         // with respect to LamellarArch used to create this object
    local_cnt: AtomicUsize, // cnt of times weve cloned for local access
    dist_cnt: AtomicUsize,  // cnt of times weve cloned (serialized) for distributed access
    ref_cnt_addr: usize,    // array of cnts for accesses from remote pes
    mode_addr: usize,
    am_counters: *const AMCounters,
    team: *const LamellarTeamRT,
    item: *const T,
}
unsafe impl<T: Sync + Send> Send for DarcInner<T> {}
unsafe impl<T: Sync + Send> Sync for DarcInner<T> {}

pub struct Darc<T: 'static> {
    inner: *mut DarcInner<T>,
    src_pe: usize,
}
unsafe impl<T: Sync + Send> Send for Darc<T> {}
unsafe impl<T: Sync + Send> Sync for Darc<T> {}

impl<T: 'static> serde::Serialize for Darc<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        __NetworkDarc::<T>::from(self).serialize(serializer)
    }
}

impl<'de, T: 'static> Deserialize<'de> for Darc<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ndarc: __NetworkDarc<T> = Deserialize::deserialize(deserializer)?;
        Ok(ndarc.into())
    }
}

impl<T> crate::DarcSerde for Darc<T> {
    fn ser(&self, num_pes: usize, _cur_pe: Result<usize, IdError>) {
        // match cur_pe {
        //     Ok(cur_pe) => {
        self.serialize_update_cnts(num_pes, 0 /*cur_pe*/);
        //     }
        //     Err(err) => {
        //         panic!("can only access darcs within team members ({:?})", err);
        //     }
        // }
    }
    fn des(&self, cur_pe: Result<usize, IdError>) {
        match cur_pe {
            Ok(cur_pe) => {
                self.deserialize_update_cnts(cur_pe);
            }
            Err(err) => {
                panic!("can only access darcs within team members ({:?})", err);
            }
        }
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
        let ref_cnt = unsafe {
            ((self.ref_cnt_addr + team_pe * std::mem::size_of::<AtomicUsize>()) as *mut AtomicUsize)
                .as_ref()
                .expect("invalid darc addr")
        };
        ref_cnt.fetch_add(amt, Ordering::SeqCst)
    }

    fn update_item(&mut self, item: *const T) {
        self.item = item;
    }

    #[allow(dead_code)]
    fn item(&self) -> &T {
        unsafe { &(*self.item) }
    }

    fn send_finished(&self) -> Vec<Pin<Box<dyn Future<Output = Option<()>> + Send>>> {
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
                // println!("sending finished to {:?} {:?} team {:?} {:x}",pe,cnt,team.team_hash,my_addr);
                // println!("{:?}",self);
                reqs.push(
                    team.exec_am_pe_tg(
                        pe,
                        FinishedAm {
                            cnt: cnt,
                            src_pe: pe,
                            inner_addr: pe_addr,
                        },
                        Some(self.am_counters()),
                    )
                    .into_future(),
                );
            }
        }
        reqs
    }
    unsafe fn any_ref_cnt(&self) -> bool {
        let ref_cnts =
            std::slice::from_raw_parts_mut(self.ref_cnt_addr as *mut usize, self.num_pes); //this is potentially a dirty read
        ref_cnts.iter().any(|x| *x > 0)
    }
    fn block_on_outstanding(&self, state: DarcMode, extra_cnt: usize) {
        self.wait_all();
        let mut timer = std::time::Instant::now();
        while self.dist_cnt.load(Ordering::SeqCst) > 0
            || self.local_cnt.load(Ordering::SeqCst) > 1 + extra_cnt
            || unsafe { self.any_ref_cnt() }
        {
            if self.local_cnt.load(Ordering::SeqCst) == 1 + extra_cnt {
                self.send_finished();
            }
            if timer.elapsed().as_secs_f64() > 10.0 {
                println!("waiting for outstanding 1 {:?}", self);
                // let rel_addr = unsafe { self as *const DarcInner<T> as usize - (*(self.team)).lamellae.base_addr() };
                // println!(
                //     "--------\norig:  {:?} (0x{:x}) {:?}\n--------",

                //     self as *const DarcInner<T>,
                //     rel_addr,
                //     self
                // );
                timer = std::time::Instant::now();
            }
            std::thread::yield_now();
        }
        let team = self.team();
        let mode_refs =
            unsafe { std::slice::from_raw_parts_mut(self.mode_addr as *mut u8, self.num_pes) };
        unsafe {
            (*(((&mut mode_refs[self.my_pe]) as *mut u8) as *mut AtomicU8)) //this should be fine given that DarcMode uses Repr(u8)
                .store(state as u8, Ordering::SeqCst)
        };
        // (&mode_refs[self.my_pe] = 2;
        let rdma = &team.lamellae;
        for pe in team.arch.team_iter() {
            rdma.put(
                pe,
                &mode_refs[self.my_pe..=self.my_pe],
                self.mode_addr + self.my_pe * std::mem::size_of::<DarcMode>(),
            );
        }
        for pe in mode_refs.iter() {
            while *pe != state as u8 {
                if self.local_cnt.load(Ordering::SeqCst) == 1 + extra_cnt {
                    self.send_finished();
                }
                if timer.elapsed().as_secs_f64() > 10.0 {
                    println!("waiting for outstanding 2 {:?}", self);
                    timer = std::time::Instant::now();
                }
                std::thread::yield_now();
            }
        }
        while self.dist_cnt.load(Ordering::SeqCst) != 0
            || self.local_cnt.load(Ordering::SeqCst) > 1 + extra_cnt
            || unsafe { self.any_ref_cnt() }
        {
            if self.local_cnt.load(Ordering::SeqCst) == 1 + extra_cnt {
                self.send_finished();
            }
            if timer.elapsed().as_secs_f64() > 10.0 {
                println!("waiting for outstanding 3 {:?}", self);
                timer = std::time::Instant::now();
            }
            std::thread::yield_now();
        }
        // println!("{:?}",self);
        self.team().barrier();
    }

    fn wait_all(&self) {
        let mut temp_now = Instant::now();
        // let mut first = true;
        let team = self.team();
        let am_counters = self.am_counters();
        while am_counters.outstanding_reqs.load(Ordering::SeqCst) > 0 {
            // std::thread::yield_now();
            team.scheduler.exec_task(); //mmight as well do useful work while we wait
            if temp_now.elapsed() > Duration::new(60, 0) {
                //|| first{
                println!(
                    "in team wait_all mype: {:?} cnt: {:?} {:?}",
                    team.world_pe,
                    am_counters.send_req_cnt.load(Ordering::SeqCst),
                    am_counters.outstanding_reqs.load(Ordering::SeqCst),
                );
                temp_now = Instant::now();
                // first = false;
            }
        }
        // println!("done in wait all {:?}",std::time::SystemTime::now());
    }
}

impl<T> fmt::Debug for DarcInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:}/{:?}] lc: {:?} dc: {:?}\nref_cnt: {:?}\n am_cnt ({:?},{:?})\nmode {:?}",
            self.my_pe,
            self.num_pes,
            self.local_cnt.load(Ordering::SeqCst),
            self.dist_cnt.load(Ordering::SeqCst),
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

    pub fn serialize_update_cnts(&self, cnt: usize, _cur_pe: usize) {
        // println!("serialize darc cnts");
        self.inner()
            .dist_cnt
            .fetch_add(cnt, std::sync::atomic::Ordering::SeqCst);
        // println!("done serialize darc cnts");
    }

    pub fn deserialize_update_cnts(&self, _cur_pe: usize) {
        // println!("deserialize darc? cnts");
        self.inner().inc_pe_ref_count(self.src_pe, 1);
        self.inner().local_cnt.fetch_add(1, Ordering::SeqCst);
        // println!{"darc deserialized {:?} {:?}",self.inner,self.inner().local_cnt.load(Ordering::SeqCst)};
        // println!("done deserialize darc cnts");
    }

    pub fn print(&self) {
        let rel_addr = unsafe { self.inner as usize - (*self.inner().team).lamellae.base_addr() };
        println!(
            "--------\norig: {:?} {:?} (0x{:x}) {:?}\n--------",
            self.src_pe,
            self.inner,
            rel_addr,
            self.inner()
        );
    }
}

impl<T> Darc<T> {
    pub fn new<U: Into<IntoLamellarTeam>>(team: U, item: T) -> Result<Darc<T>, IdError> {
        Darc::try_new(team, item, DarcMode::Darc)
    }

    pub(crate) fn try_new<U: Into<IntoLamellarTeam>>(
        team: U,
        item: T,
        state: DarcMode,
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
            + team_rt.num_pes * std::mem::size_of::<DarcMode>();
        // println!("creating new darc");
        team_rt.barrier();
        // println!("creating new darc after barrier");
        let addr = team_rt.lamellae.alloc(size, alloc).expect("out of memory");
        // let temp_team = team_rt.clone();
        let team_ptr = unsafe {
            let pinned_team = Pin::into_inner_unchecked(team_rt.clone());
            Arc::into_raw(pinned_team)
        };
        let am_counters = Arc::new(AMCounters::new());
        let am_counters_ptr = Arc::into_raw(am_counters);
        let darc_temp = DarcInner {
            my_pe: my_pe,
            num_pes: team_rt.num_pes,
            local_cnt: AtomicUsize::new(1),
            dist_cnt: AtomicUsize::new(0),
            ref_cnt_addr: addr + std::mem::size_of::<DarcInner<T>>(),
            mode_addr: addr
                + std::mem::size_of::<DarcInner<T>>()
                + team_rt.num_pes * std::mem::size_of::<usize>(),
            am_counters: am_counters_ptr,
            team: team_ptr, //&team_rt, //Arc::into_raw(temp_team),
            item: Box::into_raw(Box::new(item)),
        };
        unsafe {
            std::ptr::copy_nonoverlapping(&darc_temp, addr as *mut DarcInner<T>, 1);
        }

        let d = Darc {
            inner: addr as *mut DarcInner<T>,
            src_pe: my_pe,
        };
        for elem in d.mode_as_mut_slice() {
            *elem = state;
        }
        // d.print();
        team_rt.barrier();
        Ok(d)
    }

    pub(crate) fn block_on_outstanding(&self, state: DarcMode, extra_cnt: usize) {
        self.inner().block_on_outstanding(state, extra_cnt);
    }

    pub fn into_localrw(self) -> LocalRwDarc<T> {
        let inner = self.inner();
        let _cur_pe = inner.team().world_pe;
        inner.block_on_outstanding(DarcMode::LocalRw, 0);
        inner.local_cnt.fetch_add(1, Ordering::SeqCst); //we add this here because to account for moving inner into d
                                                        // println!{"darc into_localrw {:?} {:?}",self.inner,self.inner().local_cnt.load(Ordering::SeqCst)};
        let item = unsafe { Box::from_raw(inner.item as *mut T) };
        let d = Darc {
            inner: self.inner as *mut DarcInner<Arc<RwLock<Box<T>>>>,
            src_pe: self.src_pe,
        };
        d.inner_mut()
            .update_item(Box::into_raw(Box::new(Arc::new(RwLock::new(item)))));
        // d.print();
        LocalRwDarc { darc: d }
    }

    pub fn into_globalrw(self) -> GlobalRwDarc<T> {
        let inner = self.inner();
        let _cur_pe = inner.team().world_pe;
        inner.block_on_outstanding(DarcMode::GlobalRw, 0);
        inner.local_cnt.fetch_add(1, Ordering::SeqCst); //we add this here because to account for moving inner into d
                                                        // println!{"darc into_globalrw {:?} {:?}",self.inner,self.inner().local_cnt.load(Ordering::SeqCst)};
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
        // println!{"darc cloned {:?} {:?}",self.inner,self.inner().local_cnt.load(Ordering::SeqCst)};
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
        unsafe { &*self.inner().item }
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

impl<T: 'static> Drop for Darc<T> {
    fn drop(&mut self) {
        let inner = self.inner();
        let cnt = inner.local_cnt.fetch_sub(1, Ordering::SeqCst);
        // println!{"darc dropped {:?} {:?}",self.inner,self.inner().local_cnt.load(Ordering::SeqCst)};
        if cnt == 1 {
            //we are currently the last local ref, if it increases again it must mean someone else has come in and we can probably let them worry about cleaning up...
            let pe_ref_cnts = self.ref_cnts_as_mut_slice();
            // println!("Last local ref... for now! {:?}",pe_ref_cnts);
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

            let mode_refs = self.mode_as_mut_slice();
            let local_mode = unsafe {
                (*(((&mut mode_refs[inner.my_pe]) as *mut DarcMode) as *mut AtomicU8))
                    .compare_exchange(
                        DarcMode::Darc as u8,
                        DarcMode::Dropped as u8,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    )
            };
            if local_mode == Ok(DarcMode::Darc as u8) {
                // println!("launching drop task as darc");
                // self.print();
                let team = inner.team();
                team.exec_am_local(DroppedWaitAM {
                    inner_addr: self.inner as *const u8 as usize,
                    mode_addr: inner.mode_addr,
                    my_pe: inner.my_pe,
                    num_pes: inner.num_pes,
                    team: team.clone(),
                    phantom: PhantomData::<T>,
                });
                // println!("team cnt: {:?}", Arc::strong_count(&team));
            } else {
                let local_mode = unsafe {
                    (*(((&mut mode_refs[inner.my_pe]) as *mut DarcMode) as *mut AtomicU8))
                        .compare_exchange(
                            DarcMode::LocalRw as u8,
                            DarcMode::Dropped as u8,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                };
                if local_mode == Ok(DarcMode::LocalRw as u8) {
                    // println!("launching drop task as localrw");
                    // self.print();
                    let team = inner.team();
                    team.exec_am_local(DroppedWaitAM {
                        inner_addr: self.inner as *const u8 as usize,
                        mode_addr: inner.mode_addr,
                        my_pe: inner.my_pe,
                        num_pes: inner.num_pes,
                        team: team.clone(),
                        phantom: PhantomData::<T>,
                    });
                    // println!("team cnt: {:?}", Arc::strong_count(&team));
                } else {
                    let local_mode = unsafe {
                        (*(((&mut mode_refs[inner.my_pe]) as *mut DarcMode) as *mut AtomicU8))
                            .compare_exchange(
                                DarcMode::GlobalRw as u8,
                                DarcMode::Dropped as u8,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            )
                    };
                    if local_mode == Ok(DarcMode::GlobalRw as u8) {
                        // println!("launching drop task as globalrw");
                        // self.print();
                        let team = inner.team();
                        team.exec_am_local(DroppedWaitAM {
                            inner_addr: self.inner as *const u8 as usize,
                            mode_addr: inner.mode_addr,
                            my_pe: inner.my_pe,
                            num_pes: inner.num_pes,
                            team: team.clone(),
                            phantom: PhantomData::<T>,
                        });
                        // println!("team cnt: {:?}", Arc::strong_count(&team));
                    }
                }
            }
        }
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

unsafe impl<T> Send for DroppedWaitAM<T> {}
unsafe impl<T> Sync for DroppedWaitAM<T> {}

use std::ptr::NonNull;
struct Wrapper<T> {
    inner: NonNull<DarcInner<T>>,
}
unsafe impl<T> Send for Wrapper<T> {}

#[lamellar_impl::rt_am_local]
impl<T: 'static> LamellarAM for DroppedWaitAM<T> {
    fn exec(self) {
        // println!("in DroppedWaitAM {:x}",self.inner_addr);
        let mode_refs_u8 =
            unsafe { std::slice::from_raw_parts_mut(self.mode_addr as *mut u8, self.num_pes) };
        let mode_refs = unsafe {
            std::slice::from_raw_parts_mut(self.mode_addr as *mut DarcMode, self.num_pes)
        };

        let mut timeout = std::time::Instant::now();
        let wrapped = Wrapper {
            inner: NonNull::new(self.inner_addr as *mut DarcInner<T>)
                .expect("invalid darc pointer"),
        };
        unsafe {
            wrapped.inner.as_ref().wait_all();
            // let inner = unsafe {&*wrapped.inner}; //we dont actually care about the "type" we wrap here, we just need access to the meta data for the darc (but still allow async wait cause T is not send)
            while wrapped.inner.as_ref().dist_cnt.load(Ordering::SeqCst) != 0
                || wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst) != 0
            {
                if wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst) == 0 {
                    wrapped.inner.as_ref().send_finished();
                }
                if timeout.elapsed().as_secs_f64() > 5.0 {
                    println!(
                        "0. Darc trying to free! {:x} {:?} {:?} {:?}",
                        self.inner_addr,
                        mode_refs,
                        wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst),
                        wrapped.inner.as_ref().dist_cnt.load(Ordering::SeqCst)
                    );
                    timeout = std::time::Instant::now();
                }
                async_std::task::yield_now().await;
            }
            // let team = wrapped.inner.as_ref().team();
            let rdma = &self.team.lamellae;
            for pe in self.team.arch.team_iter() {
                // println!("putting {:?} to {:?} @ {:x}",&mode_refs[self.my_pe..=self.my_pe],pe,self.mode_addr + self.my_pe * std::mem::size_of::<u8>());
                rdma.put(
                    pe,
                    &mode_refs_u8[self.my_pe..=self.my_pe],
                    self.mode_addr + self.my_pe * std::mem::size_of::<DarcMode>(),
                );
            }
        }

        for pe in mode_refs.iter() {
            while *pe != DarcMode::Dropped {
                async_std::task::yield_now().await;
                unsafe {
                    if wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst) == 0 {
                        wrapped.inner.as_ref().send_finished();
                    }
                }
                if timeout.elapsed().as_secs_f64() > 5.0 {
                    println!(
                        "1. Darc trying to free! {:x} {:?}",
                        self.inner_addr, mode_refs
                    );
                    timeout = std::time::Instant::now();
                }
            }
        }
        // let inner =self.inner_addr as *mut DarcInner<T>;
        let wrapped = Wrapper {
            inner: NonNull::new(self.inner_addr as *mut DarcInner<T>)
                .expect("invalid darc pointer"),
        };
        // let inner = unsafe {&*wrapped.inner}; //we dont actually care about the "type" we wrap here, we just need access to the meta data for the darc (but still allow async wait cause T is not send)
        unsafe {
            while wrapped.inner.as_ref().dist_cnt.load(Ordering::SeqCst) != 0
                || wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst) != 0
            {
                if wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst) == 0 {
                    wrapped.inner.as_ref().send_finished();
                }
                if timeout.elapsed().as_secs_f64() > 5.0 {
                    println!(
                        "2. Darc trying to free! {:x} {:?} {:?} {:?}",
                        self.inner_addr,
                        mode_refs,
                        wrapped.inner.as_ref().local_cnt.load(Ordering::SeqCst),
                        wrapped.inner.as_ref().dist_cnt.load(Ordering::SeqCst)
                    );
                    timeout = std::time::Instant::now();
                }
                async_std::task::yield_now().await;
            }
            // let inner = unsafe {&*(self.inner_addr as *mut DarcInner<T>)}; //now we need to true type to deallocate appropriately
            let _item = Box::from_raw(wrapped.inner.as_ref().item as *mut T);
            let _team = Arc::from_raw(wrapped.inner.as_ref().team); //return to rust to drop appropriately
                                                                    // println!("Darc freed! {:x} {:?}",self.inner_addr,mode_refs);
            let _am_counters = Arc::from_raw(wrapped.inner.as_ref().am_counters);
            self.team.lamellae.free(self.inner_addr);
            // println!("leaving DroppedWaitAM");
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct __NetworkDarc<T> {
    inner_addr: usize,
    backend: Backend,
    orig_world_pe: usize,
    orig_team_pe: usize,
    phantom: PhantomData<T>,
}

impl<T> From<Darc<T>> for __NetworkDarc<T> {
    fn from(darc: Darc<T>) -> Self {
        // println!("net darc from darc");
        let team = &darc.inner().team();
        let ndarc = __NetworkDarc {
            inner_addr: darc.inner as *const u8 as usize,
            backend: team.lamellae.backend(),
            orig_world_pe: team.world_pe,
            orig_team_pe: team.team_pe.expect("darcs only valid on team members"),
            phantom: PhantomData,
        };
        // darc.print();
        ndarc
    }
}

impl<T> From<&Darc<T>> for __NetworkDarc<T> {
    fn from(darc: &Darc<T>) -> Self {
        // println!("net darc from darc");
        let team = &darc.inner().team();
        let ndarc = __NetworkDarc {
            inner_addr: darc.inner as *const u8 as usize,
            backend: team.lamellae.backend(),
            orig_world_pe: team.world_pe,
            orig_team_pe: team.team_pe.expect("darcs only valid on team members"),
            phantom: PhantomData,
        };
        // darc.print();
        ndarc
    }
}

impl<T> From<__NetworkDarc<T>> for Darc<T> {
    fn from(ndarc: __NetworkDarc<T>) -> Self {
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