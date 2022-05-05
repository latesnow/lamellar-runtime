mod chunks;
mod enumerate;
mod filter;
mod filter_map;
mod ignore;
mod map;
mod step_by;
mod take;
mod zip;

use chunks::*;
use enumerate::*;
use filter::*;
use filter_map::*;
use ignore::*;
use map::*;
use step_by::*;
use take::*;
use zip::*;

use crate::memregion::{Dist};
use crate::LamellarTeamRT;
// use crate::LamellarArray;
use crate::array::{UnsafeArray,AtomicArray, GenericAtomicArray, LamellarArray, NativeAtomicArray, Distribution}; //, LamellarArrayPut, LamellarArrayGet};
use crate::array::iterator::serial_iterator::SerialIterator;
use enum_dispatch::enum_dispatch;
use futures::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

#[lamellar_impl::AmLocalDataRT(Clone)]
pub(crate) struct ForEach<I, F>
where
    I: DistributedIterator,
    F: Fn(I::Item) + Sync + Send,
{
    pub(crate) op: F,
    pub(crate) data: I,
    pub(crate) start_i: usize,
    pub(crate) end_i: usize,
}
#[lamellar_impl::rt_am_local]
impl<I, F> LamellarAm for ForEach<I, F>
where
    I: DistributedIterator + 'static,
    F: Fn(I::Item) + Sync + Send + 'static,
{
    fn exec(&self) {
        // println!("in for each {:?} {:?}", self.start_i, self.end_i);
        let mut iter = self.data.init(self.start_i, self.end_i - self.start_i);
        while let Some(elem) = iter.next() {
            (&self.op)(elem)
        }
        // println!("done in for each");
    }
}

#[lamellar_impl::AmLocalDataRT(Clone)]
pub(crate) struct ForEachAsync<I, F, Fut>
where
    I: DistributedIterator,
    F: Fn(I::Item) -> Fut + Sync + Send + Clone,
    Fut: Future<Output = ()> + Send,
{
    pub(crate) op: F,
    pub(crate) data: I,
    pub(crate) start_i: usize,
    pub(crate) end_i: usize,
}
#[lamellar_impl::rt_am_local]
impl<I, F, Fut> LamellarAm for ForEachAsync<I, F, Fut>
where
    I: DistributedIterator + 'static,
    F: Fn(I::Item) -> Fut + Sync + Send  + Clone + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn exec(&self) {
        let mut iter = self.data.init(self.start_i, self.end_i - self.start_i);
        while let Some(elem) = iter.next() {
            (&self.op)(elem).await;
        }
    }
}

#[enum_dispatch]
pub trait DistIteratorLauncher {
    fn for_each<I, F>(&self, iter: I, op: F)
    //this really needs to return a task group handle...
    where
        I: DistributedIterator + 'static,
        F: Fn(I::Item) + Sync + Send + Clone + 'static;
    fn for_each_async<I, F, Fut>(&self, iter: &I, op: F)
    //this really needs to return a task group handle...
    where
        I: DistributedIterator + 'static,
        F: Fn(I::Item) -> Fut + Sync + Send  + Clone +  'static,
        Fut: Future<Output = ()> + Send +  'static;

    fn global_index_from_local(&self, index: usize, chunk_size: usize) -> Option<usize>;
    fn subarray_index_from_local(&self, index: usize, chunk_size: usize) -> Option<usize>;
    fn team(&self) -> Pin<Arc<LamellarTeamRT>>;
}

pub trait DistributedIterator: Sync + Send + Clone {
    type Item: Send;
    type Array: DistIteratorLauncher;
    fn init(&self, start_i: usize, cnt: usize) -> Self;
    fn array(&self) -> Self::Array;
    fn next(&mut self) -> Option<Self::Item>;
    fn elems(&self, in_elems: usize) -> usize;
    fn global_index(&self, index: usize) -> Option<usize>;
    fn subarray_index(&self, index: usize) -> Option<usize>;
    // fn chunk_size(&self) -> usize;
    fn advance_index(&mut self, count: usize);

    fn enumerate(self) -> Enumerate<Self> {
        Enumerate::new(self, 0)
    }
    fn filter<F>(self, op: F) -> Filter<Self,F> 
    where
        F: Fn(&Self::Item) -> bool + Sync + Send + Clone + 'static {
        Filter::new(self, op)
    }
    fn filter_map<F, R>(self, op: F) -> FilterMap<Self, F>
    where
        F: Fn(Self::Item) -> Option<R> + Sync + Send + Clone + 'static,
        R: Send + 'static,{
        FilterMap::new(self, op)
    }
    fn chunks(self, size: usize) -> Chunks<Self> {
        Chunks::new(self, 0, 0, size)
    }
    fn ignore(self, count: usize) -> Ignore<Self> {
        Ignore::new(self, count)
    }
    fn map<F, R>(self, op: F) -> Map<Self, F>
    where
        F: Fn(Self::Item) -> R + Sync + Send + Clone + 'static,
        R: Send + 'static,{
        Map::new(self, op)
    }
    fn step_by(self, step_size: usize) -> StepBy<Self> {
        StepBy::new(self, step_size)
    }
    fn take(self, count: usize) -> Take<Self> {
        Take::new(self, count)
    }
    fn zip<I: DistributedIterator>(self, iter: I) -> Zip<Self, I> {
        Zip::new(self, iter)
    }
    fn collect<A>(self,d: Distribution) -> A 
        where Self::Item: Dist, A: From<UnsafeArray<Self::Item>>  {
        let team = self.array().team();
        let local_sizes = UnsafeArray::<usize>::new(team.clone(),team.num_pes, Distribution::Block);
        let mut local_items = vec![];
        let mut iter = self.init(0, usize::MAX); //for now just iterate over the whole array here
        while let Some(item) = iter.next(){
            println!("item {:?}", item);
            local_items.push(item);
        }
        unsafe{ local_sizes.local_as_mut_slice()[0]=local_items.len(); }
        local_sizes.barrier();
        local_sizes.print();
        let mut size = 0;
        let mut my_start = 0;
        let my_pe = team.team_pe.expect("pe not part of team");
        local_sizes.ser_iter().into_iter().enumerate().for_each(|(i,local_size)| {
            size += local_size;
            if i < my_pe{
                my_start += local_size;
            }
        });
        println!{"size{} my_start {}",size,my_start};
        let array =  UnsafeArray::<Self::Item>::new(team, size, d); //implcit barrier
        array.put(my_start,&local_items);
        array.into()
    }
}

#[derive(Clone)]
pub struct DistIter<'a, T: Dist + 'static, A: LamellarArray<T> + Sync + Send> {
    data: A,
    cur_i: usize,
    end_i: usize,
    _marker: PhantomData<&'a T>,
}

impl<T: Dist, A: LamellarArray<T> + Sync + Send> DistIter<'_, T, A> {
    pub(crate) fn new(data: A, cur_i: usize, cnt: usize) -> Self {
        // println!("new dist iter {:?} {:? } {:?}",cur_i, cnt, cur_i+cnt);
        DistIter {
            data,
            cur_i,
            end_i: cur_i + cnt,
            _marker: PhantomData,
        }
    }
}

impl<
        T: Dist + 'static,
        A: LamellarArray<T> + DistIteratorLauncher + Sync + Send + Clone + 'static,
    > DistIter<'static, T, A>
{
    pub fn for_each<F>(self, op: F)
    where
        F: Fn(&T) + Sync + Send + Clone + 'static,
    {
        self.data.clone().for_each(self, op);
    }
    pub fn for_each_async<F, Fut>(&self, op: F)
    where
        F: Fn(&T) -> Fut + Sync + Send + Clone + 'static,
        Fut: Future<Output = ()> + Sync + Send + Clone + 'static,
    {
        self.data.clone().for_each_async(self, op);
    }
}

impl<'a, T: Dist + 'a, A: LamellarArray<T> + DistIteratorLauncher + Sync + Send + Clone + 'a>
    DistributedIterator for DistIter<'a, T, A>
{
    type Item = &'a T;
    type Array = A;
    fn init(&self, start_i: usize, cnt: usize) -> Self {
        let max_i = self.data.num_elems_local();
        // println!("init dist iter start_i: {:?} cnt {:?} end_i: {:?} max_i: {:?}",start_i,cnt, start_i+cnt,max_i);
        DistIter {
            data: self.data.clone(),
            cur_i: std::cmp::min(start_i, max_i),
            end_i: std::cmp::min(start_i + cnt, max_i),
            _marker: PhantomData,
        }
    }
    fn array(&self) -> Self::Array {
        self.data.clone()
    }
    fn next(&mut self) -> Option<Self::Item> {
        // println!("dist iter next cur: {:?} end {:?}",self.cur_i,self.end_i);
        if self.cur_i < self.end_i {
            self.cur_i += 1;
            unsafe {
                self.data
                    .local_as_ptr()
                    .offset((self.cur_i - 1) as isize)
                    .as_ref()
            }
        } else {
            None
        }
    }
    fn elems(&self, in_elems: usize) -> usize {
        // println!("dist iter elems {:?}",in_elems);
        in_elems
    }
    fn global_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.global_index_from_local(index, 1);
        // println!("dist_iter index: {:?} global_index {:?}", index,g_index);
        g_index
    }
    fn subarray_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.subarray_index_from_local(index, 1);
        // println!("dist_iter index: {:?} global_index {:?}", index,g_index);
        g_index
    }
    // fn chunk_size(&self) -> usize {
    //     1
    // }
    fn advance_index(&mut self, count: usize) {
        self.cur_i = std::cmp::min(self.cur_i + count, self.end_i);
    }
}

#[derive(Clone)]
pub struct DistIterMut<'a, T: Dist, A: LamellarArray<T> + Sync + Send> {
    data: A,
    cur_i: usize,
    end_i: usize,
    _marker: PhantomData<&'a T>,
}

impl<T: Dist, A: LamellarArray<T> + Sync + Send> DistIterMut<'_, T, A> {
    pub(crate) fn new(data: A, cur_i: usize, cnt: usize) -> Self {
        DistIterMut {
            data,
            cur_i,
            end_i: cur_i + cnt,
            _marker: PhantomData,
        }
    }
}

impl<
        T: Dist + 'static,
        A: LamellarArray<T> + Sync + Send + DistIteratorLauncher + Clone + 'static,
    > DistIterMut<'static, T, A>
{
    pub fn for_each<F>(self, op: F)
    where
        F: Fn(&mut T) + Sync + Send + Clone + 'static,
    {
        self.data.clone().for_each(self, op);
    }
    pub fn for_each_async<F, Fut>(&self, op: F)
    where
        F: Fn(&mut T) -> Fut + Sync + Send + Clone + 'static,
        Fut: Future<Output = ()> + Sync + Send + Clone + 'static,
    {
        self.data.clone().for_each_async(self, op);
    }
}
impl<'a, T: Dist + 'a, A: LamellarArray<T> + Sync + Send + DistIteratorLauncher + Clone>
    DistributedIterator for DistIterMut<'a, T, A>
{
    type Item = &'a mut T;
    type Array = A;
    fn init(&self, start_i: usize, cnt: usize) -> Self {
        let max_i = self.data.num_elems_local();
        // println!("dist iter init {:?} {:?} {:?}",start_i,cnt,max_i);
        DistIterMut {
            data: self.data.clone(),
            cur_i: std::cmp::min(start_i, max_i),
            end_i: std::cmp::min(start_i + cnt, max_i),
            _marker: PhantomData,
        }
    }
    fn array(&self) -> Self::Array {
        self.data.clone()
    }
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur_i < self.end_i {
            self.cur_i += 1;
            unsafe {
                Some(
                    &mut *self
                        .data
                        .local_as_mut_ptr()
                        .offset((self.cur_i - 1) as isize),
                )
            }
        } else {
            None
        }
    }
    fn elems(&self, in_elems: usize) -> usize {
        in_elems
    }
    fn global_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.global_index_from_local(index, 1);
        // println!("dist_iter index: {:?} global_index {:?}", index,g_index);
        g_index
    }
    fn subarray_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.subarray_index_from_local(index, 1);
        g_index
    }
    // fn chunk_size(&self) -> usize {
    //     1
    // }
    fn advance_index(&mut self, count: usize) {
        self.cur_i = std::cmp::min(self.cur_i + count, self.end_i);
    }
}
