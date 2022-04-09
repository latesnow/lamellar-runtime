#[cfg(not(feature = "non-buffered-array-ops"))]
pub(crate) mod buffered_operations;
pub(crate) mod iteration;
#[cfg(not(feature = "non-buffered-array-ops"))]
pub(crate) use buffered_operations as operations;
mod rdma;
use crate::array::atomic::AtomicElement;
use crate::array::generic_atomic::operations::BUFOPS;
use crate::array::private::LamellarArrayPrivate;
use crate::array::r#unsafe::UnsafeByteArray;
use crate::array::*;
use crate::darc::Darc;
use crate::darc::DarcMode;
use crate::lamellar_team::{IntoLamellarTeam, LamellarTeamRT};
use crate::memregion::Dist;
use parking_lot::{Mutex, MutexGuard};
use std::any::TypeId;
use serde::ser::SerializeSeq;
// use std::ops::{Deref, DerefMut};

use std::ops::{AddAssign, BitAndAssign, BitOrAssign, DivAssign, MulAssign, SubAssign};
pub struct GenericAtomicElement<T: Dist> {
    array: GenericAtomicArray<T>,
    local_index: usize,
}

impl<T: Dist> From<GenericAtomicElement<T>> for AtomicElement<T> {
    fn from(element: GenericAtomicElement<T>) -> AtomicElement<T> {
        AtomicElement::GenericAtomicElement(element)
    }
}

impl<T: Dist> GenericAtomicElement<T> {
    pub fn load(&self) -> T {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] }
    }
    pub fn store(&self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe {
            self.array.__local_as_mut_slice()[self.local_index] = val;
        }
    }
}
//todo does this work on sub arrays?
impl<T: Dist + ElementArithmeticOps> AddAssign<T> for GenericAtomicElement<T> {
    fn add_assign(&mut self, val: T) {
        // self.add(val)
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] += val }
    }
}

impl<T: Dist + ElementArithmeticOps> SubAssign<T> for GenericAtomicElement<T> {
    fn sub_assign(&mut self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] -= val }
    }
}

impl<T: Dist + ElementArithmeticOps> MulAssign<T> for GenericAtomicElement<T> {
    fn mul_assign(&mut self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] *= val }
    }
}

impl<T: Dist + ElementArithmeticOps> DivAssign<T> for GenericAtomicElement<T> {
    fn div_assign(&mut self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] /= val }
    }
}

impl<T: Dist + ElementBitWiseOps> BitAndAssign<T> for GenericAtomicElement<T> {
    fn bitand_assign(&mut self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] &= val }
    }
}

impl<T: Dist + ElementBitWiseOps> BitOrAssign<T> for GenericAtomicElement<T> {
    fn bitor_assign(&mut self, val: T) {
        let _lock = self.array.lock_index(self.local_index);
        unsafe { self.array.__local_as_mut_slice()[self.local_index] |= val }
    }
}

#[lamellar_impl::AmDataRT(Clone)]
pub struct GenericAtomicArray<T: Dist> {
    locks: Darc<Vec<Mutex<()>>>,
    pub(crate) array: UnsafeArray<T>,
}

#[lamellar_impl::AmDataRT(Clone)]
pub struct GenericAtomicByteArray {
    locks: Darc<Vec<Mutex<()>>>,
    pub(crate) array: UnsafeByteArray,
}

impl GenericAtomicByteArray {
    #[doc(hidden)]
    pub fn lock_index(&self, index: usize) -> MutexGuard<()> {
        let index = self
            .array
            .inner
            .pe_full_offset_for_local_index(self.array.inner.data.my_pe, index)
            .expect("invalid local index");
        self.locks[index].lock()
    }
}

#[derive(Clone)]
pub struct GenericAtomicLocalData<T: Dist> {
    array: GenericAtomicArray<T>,
    start_index: usize,
    end_index: usize,
}

pub struct GenericAtomicLocalDataIter<T: Dist> {
    array: GenericAtomicArray<T>,
    index: usize,
    end_index: usize,
}

impl<T: Dist> GenericAtomicLocalData<T> {
    pub fn at(&self, index: usize) -> GenericAtomicElement<T> {
        GenericAtomicElement {
            array: self.array.clone(),
            local_index: index,
        }
    }

    pub fn get_mut(&self, index: usize) -> Option<GenericAtomicElement<T>> {
        Some(GenericAtomicElement {
            array: self.array.clone(),
            local_index: index,
        })
    }

    pub fn len(&self) -> usize {
        unsafe { self.array.__local_as_mut_slice().len() }
    }

    pub fn iter(&self) -> GenericAtomicLocalDataIter<T> {
        GenericAtomicLocalDataIter {
            array: self.array.clone(),
            index: self.start_index,
            end_index: self.end_index,
        }
    }

    pub fn sub_data(&self, start_index: usize, end_index: usize) -> GenericAtomicLocalData<T> {
        GenericAtomicLocalData {
            array: self.array.clone(),
            start_index: start_index,
            end_index: std::cmp::min(end_index, self.array.num_elems_local()),
        }
    }
}

impl<T: Dist + serde::Serialize> serde::Serialize for GenericAtomicLocalData<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = serializer.serialize_seq(Some(self.len()))?;
        for i in 0..self.len() {
            s.serialize_element(&self.at(i).load())?;
        }
        s.end()
    }
}

impl<T: Dist> IntoIterator for GenericAtomicLocalData<T> {
    type Item = GenericAtomicElement<T>;
    type IntoIter = GenericAtomicLocalDataIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        GenericAtomicLocalDataIter {
            array: self.array,
            index: self.start_index,
            end_index: self.end_index,
        }
    }
}

impl<T: Dist> Iterator for GenericAtomicLocalDataIter<T> {
    type Item = GenericAtomicElement<T>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.end_index {
            let index = self.index;
            self.index += 1;
            Some(GenericAtomicElement {
                array: self.array.clone(),
                local_index: index,
            })
        } else {
            None
        }
    }
}

impl<T: Dist + std::default::Default> GenericAtomicArray<T> {
    //Sync + Send + Copy  == Dist
    pub fn new<U: Clone + Into<IntoLamellarTeam>>(
        team: U,
        array_size: usize,
        distribution: Distribution,
    ) -> GenericAtomicArray<T> {
        // println!("new generic_atomic array");
        let array = UnsafeArray::new(team.clone(), array_size, distribution);
        let mut vec = vec![];
        for _i in 0..array.num_elems_local() {
            vec.push(Mutex::new(()));
        }
        let locks = Darc::new(team, vec).unwrap();

        if let Some(func) = BUFOPS.get(&TypeId::of::<T>()) {
            let mut op_bufs = array.inner.data.op_buffers.write();
            let bytearray = GenericAtomicByteArray {
                locks: locks.clone(),
                array: array.clone().into(),
            };

            for pe in 0..op_bufs.len() {
                op_bufs[pe] = func(bytearray.clone());
            }
        }

        GenericAtomicArray {
            locks: locks,
            array: array,
        }
    }
}

impl<T: Dist> GenericAtomicArray<T> {
    pub(crate) fn get_element(&self, index: usize) -> GenericAtomicElement<T> {
        GenericAtomicElement {
            array: self.clone(),
            local_index: index,
        }
    }
}

impl<T: Dist> GenericAtomicArray<T> {
    pub fn wait_all(&self) {
        self.array.wait_all();
    }
    pub fn barrier(&self) {
        self.array.barrier();
    }
    pub(crate) fn num_elems_local(&self) -> usize {
        self.array.num_elems_local()
    }

    pub fn use_distribution(self, distribution: Distribution) -> Self {
        GenericAtomicArray {
            locks: self.locks.clone(),
            array: self.array.use_distribution(distribution),
        }
    }

    pub fn num_pes(&self) -> usize {
        self.array.num_pes()
    }

    #[doc(hidden)]
    pub fn pe_for_dist_index(&self, index: usize) -> Option<usize> {
        self.array.pe_for_dist_index(index)
    }

    #[doc(hidden)]
    pub fn pe_offset_for_dist_index(&self, pe: usize, index: usize) -> Option<usize> {
        self.array.pe_offset_for_dist_index(pe, index)
    }

    pub fn len(&self) -> usize {
        self.array.len()
    }

    pub fn local_data(&self) -> GenericAtomicLocalData<T> {
        GenericAtomicLocalData {
            array: self.clone(),
            start_index: 0,
            end_index: self.array.num_elems_local(),
        }
    }

    pub fn mut_local_data(&self) -> GenericAtomicLocalData<T> {
        GenericAtomicLocalData {
            array: self.clone(),
            start_index: 0,
            end_index: self.array.num_elems_local(),
        }
    }

    #[doc(hidden)]
    pub unsafe fn __local_as_slice(&self) -> &[T] {
        self.array.local_as_mut_slice()
    }
    #[doc(hidden)]
    pub unsafe fn __local_as_mut_slice(&self) -> &mut [T] {
        self.array.local_as_mut_slice()
    }

    pub fn sub_array<R: std::ops::RangeBounds<usize>>(&self, range: R) -> Self {
        GenericAtomicArray {
            locks: self.locks.clone(),
            array: self.array.sub_array(range),
        }
    }

    pub fn into_unsafe(self) -> UnsafeArray<T> {
        self.array.into()
    }

    pub fn into_local_only(self) -> LocalOnlyArray<T> {
        self.array.into()
    }

    pub fn into_read_only(self) -> ReadOnlyArray<T> {
        self.array.into()
    }

    #[doc(hidden)]
    pub fn lock_index(&self, index: usize) -> MutexGuard<()> {
        // if let Some(ref locks) = *self.locks {
        //     let start_index = (index * std::mem::size_of::<T>()) / self.orig_t_size;
        //     let end_index = ((index + 1) * std::mem::size_of::<T>()) / self.orig_t_size;
        //     let mut guards = vec![];
        //     for i in start_index..end_index {
        //         guards.push(locks[i].lock())
        //     }
        //     Some(guards)
        // } else {
        //     None
        // }
        // println!("trying to lock {:?}",index);
        let index = self
            .array
            .inner
            .pe_full_offset_for_local_index(self.array.inner.data.my_pe, index)
            .expect("invalid local index");
        self.locks[index].lock()
    }
}

impl<T: Dist + 'static> GenericAtomicArray<T> {
    pub fn into_atomic(self) -> GenericAtomicArray<T> {
        self.array.into()
    }
}

impl<T: Dist> From<UnsafeArray<T>> for GenericAtomicArray<T> {
    fn from(array: UnsafeArray<T>) -> Self {
        array.block_on_outstanding(DarcMode::GenericAtomicArray);
        let mut vec = vec![];
        for _i in 0..array.num_elems_local() {
            vec.push(Mutex::new(()));
        }
        let locks = Darc::new(array.team(), vec).unwrap();
        if let Some(func) = BUFOPS.get(&TypeId::of::<T>()) {
            let bytearray = GenericAtomicByteArray {
                locks: locks.clone(),
                array: array.clone().into(),
            };
            let mut op_bufs = array.inner.data.op_buffers.write();
            for _pe in 0..array.inner.data.num_pes {
                op_bufs.push(func(bytearray.clone()))
            }
        }
        GenericAtomicArray {
            locks: locks,
            array: array,
        }
    }
}

impl<T: Dist> From<GenericAtomicArray<T>> for GenericAtomicByteArray {
    fn from(array: GenericAtomicArray<T>) -> Self {
        GenericAtomicByteArray {
            locks: array.locks.clone(),
            array: array.array.into(),
        }
    }
}
impl<T: Dist> From<GenericAtomicArray<T>> for AtomicByteArray {
    fn from(array: GenericAtomicArray<T>) -> Self {
        AtomicByteArray::GenericAtomicByteArray(GenericAtomicByteArray {
            locks: array.locks.clone(),
            array: array.array.into(),
        })
    }
}
impl<T: Dist> From<GenericAtomicByteArray> for GenericAtomicArray<T> {
    fn from(array: GenericAtomicByteArray) -> Self {
        GenericAtomicArray {
            locks: array.locks.clone(),
            array: array.array.into(),
        }
    }
}
impl<T: Dist> From<GenericAtomicByteArray> for AtomicArray<T> {
    fn from(array: GenericAtomicByteArray) -> Self {
        GenericAtomicArray {
            locks: array.locks.clone(),
            array: array.array.into(),
        }
        .into()
    }
}

impl<T: Dist> private::ArrayExecAm<T> for GenericAtomicArray<T> {
    fn team(&self) -> Pin<Arc<LamellarTeamRT>> {
        self.array.team().clone()
    }
    fn team_counters(&self) -> Arc<AMCounters> {
        self.array.team_counters()
    }
}

impl<T: Dist> private::LamellarArrayPrivate<T> for GenericAtomicArray<T> {
    fn local_as_ptr(&self) -> *const T {
        self.array.local_as_mut_ptr()
    }
    fn local_as_mut_ptr(&self) -> *mut T {
        self.array.local_as_mut_ptr()
    }
    fn pe_for_dist_index(&self, index: usize) -> Option<usize> {
        self.array.pe_for_dist_index(index)
    }
    fn pe_offset_for_dist_index(&self, pe: usize, index: usize) -> Option<usize> {
        self.array.pe_offset_for_dist_index(pe, index)
    }
    unsafe fn into_inner(self) -> UnsafeArray<T> {
        self.array
    }
}

impl<T: Dist> LamellarArray<T> for GenericAtomicArray<T> {
    fn my_pe(&self) -> usize {
        self.array.my_pe()
    }
    fn team(&self) -> Pin<Arc<LamellarTeamRT>> {
        self.array.team().clone()
    }
    fn num_elems_local(&self) -> usize {
        self.num_elems_local()
    }
    fn len(&self) -> usize {
        self.len()
    }
    fn barrier(&self) {
        self.barrier();
    }
    fn wait_all(&self) {
        self.array.wait_all()
        // println!("done in wait all {:?}",std::time::SystemTime::now());
    }
    fn pe_and_offset_for_global_index(&self, index: usize) -> Option<(usize, usize)> {
        self.array.pe_and_offset_for_global_index(index)
    }
}

impl<T: Dist> LamellarWrite for GenericAtomicArray<T> {}
impl<T: Dist> LamellarRead for GenericAtomicArray<T> {}

impl<T: Dist> SubArray<T> for GenericAtomicArray<T> {
    type Array = GenericAtomicArray<T>;
    fn sub_array<R: std::ops::RangeBounds<usize>>(&self, range: R) -> Self::Array {
        self.sub_array(range).into()
    }
    fn global_index(&self, sub_index: usize) -> usize {
        self.array.global_index(sub_index)
    }
}

impl<T: Dist + std::fmt::Debug> GenericAtomicArray<T> {
    pub fn print(&self) {
        self.array.print();
    }
}

impl<T: Dist + std::fmt::Debug> ArrayPrint<T> for GenericAtomicArray<T> {
    fn print(&self) {
        self.array.print()
    }
}

impl<T: Dist + AmDist + 'static> GenericAtomicArray<T> {
    pub fn reduce(&self, op: &str) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        self.array.reduce(op)
    }
    pub fn sum(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        self.array.reduce("sum")
    }
    pub fn prod(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        self.array.reduce("prod")
    }
    pub fn max(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        self.array.reduce("max")
    }
}

// impl<T: Dist + serde::ser::Serialize + serde::de::DeserializeOwned + 'static> LamellarArrayReduce<T>
//     for GenericAtomicArray<T>
// {
//     fn get_reduction_op(&self, op: String) -> LamellarArcAm {
//         self.array.get_reduction_op(op)
//     }
//     fn reduce(&self, op: &str) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
//         self.reduce(op)
//     }
//     fn sum(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
//         self.sum()
//     }
//     fn max(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
//         self.max()
//     }
//     fn prod(&self) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
//         self.prod()
//     }
// }
