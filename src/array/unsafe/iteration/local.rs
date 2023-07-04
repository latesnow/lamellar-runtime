use crate::active_messaging::SyncSend;
// use crate::array::iterator::local_iterator::LocalIterForEachHandle;
// use crate::array::iterator::local_iterator::{LocalIterator,IterSchedule,IterWorkStealer};
use crate::array::iterator::local_iterator::*;
use crate::array::r#unsafe::UnsafeArray;
use crate::array::LamellarArray;

use crate::memregion::Dist;
use crate::array::iterator::Schedule;
use crate::lamellar_team::LamellarTeamRT;

use core::marker::PhantomData;
use futures::Future;
use parking_lot::Mutex;
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

impl<T: Dist> UnsafeArray<T> {
    fn local_sched_static<C,AmO,O>(
        &self,
        cons: C,
    ) -> Pin<Box<dyn Future<Output = O> + Send>>
    where
        C: IterConsumer<AmOutput=AmO, Output=O>,
        AmO: SyncSend + 'static,
        O: SyncSend + 'static,{
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = cons.max_elems(self.num_elems_local());
            let elems_per_thread = 1.0f64.max(num_elems_local as f64 / num_workers as f64);
            let mut worker = 0;
            while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
                let start_i = (worker as f64 * elems_per_thread).round() as usize;
                let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
                reqs.push(self.inner.data.task_group.exec_arc_am_local_inner(
                    cons.clone().into_am(IterSchedule::Static(start_i,end_i))
                ));
                
                worker += 1;
            }
        }
        cons.create_handle(self.inner.data.team.clone(),reqs).into_future()
    }

    fn local_sched_dynamic<C,AmO,O>(
        &self,
        cons: C,
    ) -> Pin<Box<dyn Future<Output = O> + Send>>
    where
        C: IterConsumer<AmOutput=AmO, Output=O>,
        AmO: SyncSend + 'static,
        O: SyncSend + 'static,{
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = cons.max_elems(self.num_elems_local());

            let cur_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_elems_local) {
                reqs.push(self.inner.data.task_group.exec_arc_am_local_inner(
                    cons.clone().into_am(IterSchedule::Dynamic(cur_i.clone(),num_elems_local))
                ));
            }
        }
        cons.create_handle(self.inner.data.team.clone(),reqs).into_future()
    }

    fn local_sched_work_stealing<C,AmO,O>(
        &self,
        cons: C,
    ) -> Pin<Box<dyn Future<Output = O> + Send>>
    where
        C: IterConsumer<AmOutput=AmO, Output=O>,
        AmO: SyncSend + 'static,
        O: SyncSend + 'static,{
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = cons.max_elems(self.num_elems_local());
            let elems_per_thread = 1.0f64.max(num_elems_local as f64 / num_workers as f64);
            // println!(
            //     "num_chunks {:?} chunks_thread {:?}",
            //     num_elems_local, elems_per_thread
            // );
            let mut worker = 0;
            let mut siblings = Vec::new();
            while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
                let start_i = (worker as f64 * elems_per_thread).round() as usize;
                let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
                siblings.push(IterWorkStealer {
                    range: Arc::new(Mutex::new((start_i, end_i))),
                });
                worker += 1;
            }
            for sibling in &siblings {
                reqs.push(self.inner.data.task_group.exec_arc_am_local_inner(
                    cons.clone().into_am(IterSchedule::WorkStealing(sibling.clone(),siblings.clone()))
                ))
            }
        }
        cons.create_handle(self.inner.data.team.clone(),reqs).into_future()
    }

    fn local_sched_guided<C,AmO,O>(
        &self,
        cons: C,
    ) -> Pin<Box<dyn Future<Output = O> + Send>>
    where
        C: IterConsumer<AmOutput=AmO, Output=O>,
        AmO: SyncSend + 'static,
        O: SyncSend + 'static,{
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local_orig = cons.max_elems(self.num_elems_local());
            let mut num_elems_local = num_elems_local_orig as f64;
            let mut elems_per_thread = num_elems_local / num_workers as f64;
            let mut ranges = Vec::new();
            let mut cur_i = 0;
            let mut i;
            while elems_per_thread > 100.0 && cur_i < num_elems_local_orig {
                num_elems_local = num_elems_local / 1.61; //golden ratio
                let start_i = cur_i;
                let end_i = std::cmp::min(
                    cur_i + num_elems_local.round() as usize,
                    num_elems_local_orig,
                );
                i = 0;
                while cur_i < end_i {
                    ranges.push((
                        start_i + (i as f64 * elems_per_thread).round() as usize,
                        start_i + ((i + 1) as f64 * elems_per_thread).round() as usize,
                    ));
                    i += 1;
                    cur_i = start_i + (i as f64 * elems_per_thread).round() as usize;
                }
                elems_per_thread = num_elems_local / num_workers as f64;
            }
            if elems_per_thread < 1.0 {
                elems_per_thread = 1.0;
            }
            i = 0;
            let start_i = cur_i;
            while cur_i < num_elems_local_orig {
                ranges.push((
                    start_i + (i as f64 * elems_per_thread).round() as usize,
                    start_i + ((i + 1) as f64 * elems_per_thread).round() as usize,
                ));
                i += 1;
                cur_i = start_i + (i as f64 * elems_per_thread).round() as usize;
            }
            let range_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_elems_local_orig) {
                reqs.push(self.inner.data.task_group.exec_arc_am_local_inner(
                    cons.clone().into_am(IterSchedule::Chunk(ranges.clone(),range_i.clone()))
                ));
            }
        }
        cons.create_handle(self.inner.data.team.clone(),reqs).into_future()
    }

    fn local_sched_chunk<C,AmO,O>(
        &self,
        cons: C,
        chunk_size: usize,
    ) -> Pin<Box<dyn Future<Output = O> + Send>>
    where
        C: IterConsumer<AmOutput=AmO, Output=O>,
        AmO: SyncSend + 'static,
        O: SyncSend + 'static,{
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = cons.max_elems(self.num_elems_local());
            let mut ranges = Vec::new();
            let mut cur_i = 0;
            let mut num_chunks = 0;
            while cur_i < num_elems_local {
                ranges.push((cur_i, cur_i + chunk_size));
                cur_i += chunk_size;
                num_chunks += 1;
            }

            let range_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_chunks) {
                reqs.push(self.inner.data.task_group.exec_arc_am_local_inner(
                    cons.clone().into_am(IterSchedule::Chunk(ranges.clone(),range_i.clone()))
                ));
            }
        }
        cons.create_handle(self.inner.data.team.clone(),reqs).into_future()
    }

    fn local_for_each_async_static<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = iter.elems(self.num_elems_local());
            let elems_per_thread = 1.0f64.max(num_elems_local as f64 / num_workers as f64);
            // println!(
            //     "num_chunks {:?} chunks_thread {:?}",
            //     num_elems_local, elems_per_thread
            // );
            let mut worker = 0;
            let iter = iter.init(0, num_elems_local);
            while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
                let start_i = (worker as f64 * elems_per_thread).round() as usize;
                let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
                reqs.push(self.inner.data.task_group.exec_am_local_inner(
                    ForEachAsyncStatic {
                        op: op.clone(),
                        data: iter.clone(),
                        start_i: start_i,
                        end_i: end_i,
                    },
                ));
                worker += 1;
            }
        }
        Box::new(LocalIterForEachHandle { reqs: reqs }).into_future()
    }

    fn local_for_each_async_dynamic<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = iter.elems(self.num_elems_local());

            let cur_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_elems_local) {
                reqs.push(self.inner.data.task_group.exec_am_local_inner(
                    ForEachAsyncDynamic {
                        op: op.clone(),
                        data: iter.clone(),
                        cur_i: cur_i.clone(),
                        max_i: num_elems_local,
                    },
                ));
            }
        }
        Box::new(LocalIterForEachHandle { reqs: reqs }).into_future()
    }

    fn local_for_each_async_work_stealing<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = iter.elems(self.num_elems_local());
            let elems_per_thread = 1.0f64.max(num_elems_local as f64 / num_workers as f64);
            // println!(
            //     "num_chunks {:?} chunks_thread {:?}",
            //     num_elems_local, elems_per_thread
            // );
            let mut worker = 0;
            let mut siblings = Vec::new();
            while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
                let start_i = (worker as f64 * elems_per_thread).round() as usize;
                let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
                siblings.push(IterWorkStealer {
                    range: Arc::new(Mutex::new((start_i, end_i))),
                });
                worker += 1;
            }
            for sibling in &siblings {
                reqs.push(self.inner.data.task_group.exec_am_local_inner(
                    ForEachAsyncWorkStealing {
                        op: op.clone(),
                        data: iter.clone(),
                        range: sibling.clone(),
                        siblings: siblings.clone(),
                    },
                ));
            }
        }
        Box::new(LocalIterForEachHandle { reqs: reqs }).into_future()
    }

    fn local_for_each_async_guided<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local_orig = iter.elems(self.num_elems_local());
            let mut num_elems_local = num_elems_local_orig as f64;
            let mut elems_per_thread = num_elems_local / num_workers as f64;
            let mut ranges = Vec::new();
            let mut cur_i = 0;
            let mut i;
            while elems_per_thread > 100.0 && cur_i < num_elems_local_orig {
                num_elems_local = num_elems_local / 1.61; //golden ratio
                let start_i = cur_i;
                let end_i = std::cmp::min(
                    cur_i + num_elems_local.round() as usize,
                    num_elems_local_orig,
                );
                i = 0;
                while cur_i < end_i {
                    ranges.push((
                        start_i + (i as f64 * elems_per_thread).round() as usize,
                        start_i + ((i + 1) as f64 * elems_per_thread).round() as usize,
                    ));
                    i += 1;
                    cur_i = start_i + (i as f64 * elems_per_thread).round() as usize;
                }
                elems_per_thread = num_elems_local / num_workers as f64;
            }
            if elems_per_thread < 1.0 {
                elems_per_thread = 1.0;
            }
            i = 0;
            let start_i = cur_i;
            while cur_i < num_elems_local_orig {
                ranges.push((
                    start_i + (i as f64 * elems_per_thread).round() as usize,
                    start_i + ((i + 1) as f64 * elems_per_thread).round() as usize,
                ));
                i += 1;
                cur_i = start_i + (i as f64 * elems_per_thread).round() as usize;
            }
            let range_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_elems_local_orig) {
                reqs.push(self.inner.data.task_group.exec_am_local_inner(
                    ForEachAsyncChunk {
                        op: op.clone(),
                        data: iter.clone(),
                        ranges: ranges.clone(),
                        range_i: range_i.clone(),
                    },
                ));
            }
        }
        Box::new(LocalIterForEachHandle { reqs: reqs }).into_future()
    }

    fn local_for_each_async_chunk<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
        chunk_size: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut reqs = Vec::new();
        if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
            let num_workers = self.inner.data.team.num_threads();
            let num_elems_local = iter.elems(self.num_elems_local());
            let mut ranges = Vec::new();
            let mut cur_i = 0;
            while cur_i < num_elems_local {
                ranges.push((cur_i, cur_i + chunk_size));
                cur_i += chunk_size;
            }

            let range_i = Arc::new(AtomicUsize::new(0));
            // println!("ranges {:?}", ranges);
            for _ in 0..std::cmp::min(num_workers, num_elems_local) {
                reqs.push(self.inner.data.task_group.exec_am_local_inner(
                    ForEachAsyncChunk {
                        op: op.clone(),
                        data: iter.clone(),
                        ranges: ranges.clone(),
                        range_i: range_i.clone(),
                    },
                ));
            }
        }
        Box::new(LocalIterForEachHandle { reqs: reqs }).into_future()
    }

    // fn local_reduce_static<I, F>(
    //     &self,
    //     iter: &I,
    //     op: F,
    // ) -> Pin<Box<dyn Future<Output =I::Item > + Send>>
    // where
    //     I: LocalIterator + 'static,
    //     I::Item: SyncSend,
    //     F: Fn(I::Item, I::Item) -> I::Item + SyncSend + Clone + 'static,
    // {
    //     let mut reqs = Vec::new();
    //     if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
    //         let num_workers = match std::env::var("LAMELLAR_THREADS") {
    //             Ok(n) => n.parse::<usize>().unwrap(),
    //             Err(_) => 4,
    //         };
    //         let num_elems_local = iter.elems(self.num_elems_local());
    //         let elems_per_thread = 1.0f64.max(num_elems_local as f64 / num_workers as f64);

    //         // println!(
    //         //     "num_chunks {:?} chunks_thread {:?}",
    //         //     num_elems_local, elems_per_thread
    //         // );
    //         let mut worker = 0;
    //         let iter = iter.init(0, num_elems_local);
    //         while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
    //             let start_i = (worker as f64 * elems_per_thread).round() as usize;
    //             let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
    //             reqs.push(self.inner.data.task_group.exec_am_local_inner(
    //                 ReduceStatic {
    //                     op: op.clone(),
    //                     data: iter.clone(),
    //                     start_i: start_i,
    //                     end_i: end_i,
    //                 },
    //             ));
    //             worker += 1;
    //         }
    //     }
    //     Box::new(LocalIterReduceHandle { reqs: reqs, op: op }).into_future()
    // }
}

impl<T: Dist> LocalIteratorLauncher for UnsafeArray<T> {
    fn local_global_index_from_local(&self, index: usize, chunk_size: usize) -> Option<usize> {
        // println!("global index cs:{:?}",chunk_size);
        if chunk_size == 1 {
            self.inner.global_index_from_local(index)
        } else {
            Some(self.inner.global_index_from_local(index * chunk_size)? / chunk_size)
        }
    }

    fn local_subarray_index_from_local(&self, index: usize, chunk_size: usize) -> Option<usize> {
        if chunk_size == 1 {
            self.inner.subarray_index_from_local(index)
        } else {
            Some(self.inner.subarray_index_from_local(index * chunk_size)? / chunk_size)
        }
    }

    fn local_for_each<I, F>(&self, iter: &I, op: F) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) + SyncSend + Clone + 'static,
    {
        self.local_for_each_with_schedule(Schedule::Static, iter, op)
    }

    fn local_for_each_with_schedule<I, F>(
        &self,
        sched: Schedule,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) + SyncSend + Clone + 'static,
    {
        let for_each = ForEach{
            iter: iter.clone(),
            op,
        };
        match sched {
            Schedule::Static => self.local_sched_static(for_each ),
            Schedule::Dynamic => self.local_sched_dynamic(for_each),
            Schedule::Chunk(size) => self.local_sched_chunk(for_each, size),
            Schedule::Guided => self.local_sched_guided(for_each),
            Schedule::WorkStealing => self.local_sched_work_stealing(for_each),
        }
    }

    fn local_for_each_async<I, F, Fut>(
        &self,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.local_for_each_async_static(iter, op)
    }

    fn local_for_each_async_with_schedule<I, F, Fut>(
        &self,
        sched: Schedule,
        iter: &I,
        op: F,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        I: LocalIterator + 'static,
        F: Fn(I::Item) -> Fut + SyncSend + Clone + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        match sched {
            Schedule::Static => self.local_for_each_async_static(iter, op),
            Schedule::Dynamic => self.local_for_each_async_dynamic(iter, op),
            Schedule::Chunk(size) => self.local_for_each_async_chunk(iter, op, size),
            Schedule::Guided => self.local_for_each_async_guided(iter, op),
            Schedule::WorkStealing => self.local_for_each_async_work_stealing(iter, op),
        }
    }

    // fn local_reduce<I, F>(&self, iter: &I, op: F) -> Pin<Box<dyn Future<Output = I::Item> + Send>>
    // where
    //     I: LocalIterator + 'static,
    //     I::Item: SyncSend,
    //     F: Fn(I::Item, I::Item) -> I::Item + SyncSend + Clone + 'static,
    // {
    //     self.local_sched_static(iter, IterConsumer::Reduce(op))
    // }

    // fn local_collect<I, A>(&self, iter: &I, d: Distribution) -> Pin<Box<dyn Future<Output = A> + Send>>
    // where
    //     I:  LocalIterator + 'static,
    //     I::Item: Dist,
    //     A: From<UnsafeArray<I::Item>> + SyncSend + 'static,
    // {
    //     let mut reqs = Vec::new();
    //     if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
    //         let num_workers =  self.inner.data.team.num_threads();
    //         let num_elems_local = iter.elems(self.num_elems_local());
    //         let elems_per_thread = num_elems_local as f64 / num_workers as f64;
    //         // println!("num_chunks {:?} chunks_thread {:?}", num_elems_local, elems_per_thread);
    //         let mut worker = 0;
    //         let iter = iter.init(0, num_elems_local);
    //         while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
    //             let start_i = (worker as f64 * elems_per_thread).round() as usize;
    //             let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
    //             reqs.push(self.inner.data.task_group.exec_am_local_inner(LocalCollect {
    //                 data: iter.clone(),
    //                 start_i: start_i,
    //                 end_i: end_i,
    //             }));
    //             worker += 1;
    //         }
    //     }
    //     Box::new(LocalIterCollectHandle {
    //         reqs: reqs,
    //         distribution: d,
    //         team: self.inner.data.team.clone(),
    //         _phantom: PhantomData,
    //     })
    //     .into_future()
    // }

    // fn local_collect_async<I, A, B>(
    //     &self,
    //     iter: &I,
    //     d: Distribution,
    // ) -> Pin<Box<dyn Future<Output = A> + Send>>
    // where
    //     I:  LocalIterator + 'static,
    //     I::Item: Future<Output = B> + Send + 'static,
    //     B: Dist,
    //     A: From<UnsafeArray<B>> + SyncSend + 'static,
    // {
    //     let mut reqs = Vec::new();
    //     if let Ok(_my_pe) = self.inner.data.team.team_pe_id() {
    //         let num_workers =  self.inner.data.team.num_threads();
    //         let num_elems_local = iter.elems(self.num_elems_local());
    //         let elems_per_thread = num_elems_local as f64 / num_workers as f64;
    //         println!(
    //             "num_chunks {:?} chunks_thread {:?}",
    //             num_elems_local, elems_per_thread
    //         );
    //         let mut worker = 0;
    //         let iter = iter.init(0, num_elems_local);
    //         while ((worker as f64 * elems_per_thread).round() as usize) < num_elems_local {
    //             let start_i = (worker as f64 * elems_per_thread).round() as usize;
    //             let end_i = ((worker + 1) as f64 * elems_per_thread).round() as usize;
    //             reqs.push(
    //                 self.inner
    //                     .data
    //                     .task_group
    //                     .exec_am_local_inner(LocalCollectAsync {
    //                         data: iter.clone(),
    //                         start_i: start_i,
    //                         end_i: end_i,
    //                         _phantom: PhantomData,
    //                     }),
    //             );
    //             worker += 1;
    //         }
    //     }
    //     Box::new(LocalIterCollectHandle {
    //         reqs: reqs,
    //         distribution: d,
    //         team: self.inner.data.team.clone(),
    //         _phantom: PhantomData,
    //     })
    //     .into_future()
    // }

    fn team(&self) -> Pin<Arc<LamellarTeamRT>> {
        self.inner.data.team.clone()
    }
}