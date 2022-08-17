use crate::array::atomic::*;

use crate::array::iterator::distributed_iterator::{DistIteratorLauncher, DistributedIterator};
use crate::array::iterator::serial_iterator::LamellarArrayIter;
use crate::array::*;
use crate::memregion::Dist;

#[derive(Clone)]
pub struct AtomicDistIter<T: Dist> {
    //dont need a AtomicDistIterMut in this case as any updates to inner elements are atomic
    data: AtomicArray<T>,
    cur_i: usize,
    end_i: usize,
}

impl<T: Dist> std::fmt::Debug for AtomicDistIter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AtomicDistIter{{ data.len: {:?}, cur_i: {:?}, end_i: {:?} }}",
            self.data.len(),
            self.cur_i,
            self.end_i
        )
    }
}

impl<T: Dist> AtomicDistIter<T> {
    pub(crate) fn new(data: AtomicArray<T>, cur_i: usize, cnt: usize) -> Self {
        // println!("new dist iter {:?} {:? } {:?}",cur_i, cnt, cur_i+cnt);
        AtomicDistIter {
            data,
            cur_i,
            end_i: cur_i + cnt,
        }
    }
}
// impl<T: Dist + 'static> AtomicDistIter<T> {
//     pub fn for_each<F>(&self, op: F) -> DistIterForEachHandle
//     where
//         F: Fn(AtomicElement<T>) + AmLocal + Clone + 'static,
//     {
//         self.data.clone().for_each(self, op)
//     }
//     pub fn for_each_async<F, Fut>(&self, op: F) -> DistIterForEachHandle
//     where
//         F: Fn(AtomicElement<T>) -> Fut + AmLocal + Clone + 'static,
//         Fut: Future<Output = ()> + AmLocal + Clone + 'static,
//     {
//         self.data.clone().for_each_async(self, op)
//     }
// }

impl<T: Dist> DistributedIterator for AtomicDistIter<T> {
    type Item = AtomicElement<T>;
    type Array = AtomicArray<T>;
    fn init(&self, start_i: usize, cnt: usize) -> Self {
        let max_i = self.data.num_elems_local();
        // println!("init dist iter start_i: {:?} cnt {:?} end_i: {:?} max_i: {:?}",start_i,cnt, start_i+cnt,max_i);
        // println!("num_elems_local: {:?}",self.data.num_elems_local());
        AtomicDistIter {
            data: self.data.clone(),
            cur_i: std::cmp::min(start_i, max_i),
            end_i: std::cmp::min(start_i + cnt, max_i),
        }
    }
    fn array(&self) -> Self::Array {
        self.data.clone()
    }
    fn next(&mut self) -> Option<Self::Item> {
        // println!("{:?} {:?}",self.cur_i,self.end_i);
        if self.cur_i < self.end_i {
            self.cur_i += 1;
            Some(self.data.get_element(self.cur_i - 1))
        } else {
            None
        }
    }
    fn elems(&self, in_elems: usize) -> usize {
        in_elems
    }
    fn global_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.global_index_from_local(index, 1);
        g_index
    }
    fn subarray_index(&self, index: usize) -> Option<usize> {
        let g_index = self.data.subarray_index_from_local(index, 1);
        g_index
    }
    fn advance_index(&mut self, count: usize) {
        self.cur_i = std::cmp::min(self.cur_i + count, self.end_i);
    }
}

impl<T: Dist> AtomicArray<T> {
    pub fn dist_iter(&self) -> AtomicDistIter<T> {
        AtomicDistIter::new(self.clone(), 0, 0)
    }

    pub fn dist_iter_mut(&self) -> AtomicDistIter<T> {
        AtomicDistIter::new(self.clone(), 0, 0)
    }
}

impl<T: Dist> AtomicArray<T> {
    pub fn ser_iter(&self) -> LamellarArrayIter<'_, T, AtomicArray<T>> {
        LamellarArrayIter::new(self.clone().into(), LamellarArray::team(self).clone(), 1)
    }

    pub fn buffered_iter(&self, buf_size: usize) -> LamellarArrayIter<'_, T, AtomicArray<T>> {
        LamellarArrayIter::new(
            self.clone().into(),
            LamellarArray::team(self).clone(),
            std::cmp::min(buf_size, self.len()),
        )
    }
}
