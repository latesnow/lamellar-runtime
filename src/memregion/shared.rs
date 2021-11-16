use crate::darc::Darc;
use crate::lamellae::AllocationType;
use crate::memregion::*;
use core::marker::PhantomData;
#[cfg(feature = "enable-prof")]
use lamellar_prof::*;
use std::sync::Arc;

use std::ops::Bound;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct SharedMemoryRegion<T: Arraydist> {
    pub(crate) mr: Darc<MemoryRegion<u8>>,
    sub_region_offset: usize,
    sub_region_size: usize,
    phantom: PhantomData<T>,
}

impl<T: Arraydist> crate::DarcSerde for SharedMemoryRegion<T> {
    ///hmmm why do I need to implement manually, I think i would work with the macro automatically now?
    fn ser(&self, num_pes: usize, cur_pe: Result<usize, crate::IdError>) {
        // println!("in shared ser");
        match cur_pe {
            Ok(cur_pe) => {
                self.mr.serialize_update_cnts(num_pes, cur_pe);
            }
            Err(err) => {
                panic!("can only access darcs within team members ({:?})", err);
            }
        }
    }
    fn des(&self, cur_pe: Result<usize, crate::IdError>) {
        // println!("in shared des");
        match cur_pe {
            Ok(cur_pe) => {
                self.mr.deserialize_update_cnts(cur_pe);
            }
            Err(err) => {
                panic!("can only access darcs within team members ({:?})", err);
            }
        }
        // self.mr.print();
    }
}

impl<T: Arraydist> SharedMemoryRegion<T> {
    pub(crate) fn new(
        size: usize,
        team: Arc<LamellarTeamRT>,
        alloc: AllocationType,
    ) -> SharedMemoryRegion<T> {
        SharedMemoryRegion::try_new(size, team, alloc).expect("Out of memory")
    }

    pub(crate) fn try_new(
        size: usize,
        team: Arc<LamellarTeamRT>,
        alloc: AllocationType,
    ) -> Result<SharedMemoryRegion<T>, anyhow::Error> {
        Ok(SharedMemoryRegion {
            mr: Darc::try_new(
                team.clone(),
                MemoryRegion::try_new(
                    size * std::mem::size_of::<T>(),
                    team.lamellae.clone(),
                    alloc,
                )?,
                crate::darc::DarcMode::Darc,
            )
            .expect("memregions can only be created on a member of the team"),
            sub_region_offset: 0,
            sub_region_size: size,
            phantom: PhantomData,
        })
    }
    pub fn len(&self) -> usize {
        RegisteredMemoryRegion::<T>::len(self)
    }
    pub unsafe fn put<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        MemoryRegionRDMA::<T>::put(self, pe, index, data);
    }
    pub fn iput<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        MemoryRegionRDMA::<T>::iput(self, pe, index, data);
    }
    pub unsafe fn put_all<U: Into<LamellarMemoryRegion<T>>>(&self, index: usize, data: U) {
        MemoryRegionRDMA::<T>::put_all(self, index, data);
    }
    pub unsafe fn get<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        MemoryRegionRDMA::<T>::get(self, pe, index, data);
    }
    pub fn sub_region<R: std::ops::RangeBounds<usize>>(&self, range: R) -> LamellarMemoryRegion<T> {
        SubRegion::<T>::sub_region(self, range)
    }
    pub fn as_slice(&self) -> MemResult<&[T]> {
        RegisteredMemoryRegion::<T>::as_slice(self)
    }
    pub unsafe fn as_mut_slice(&self) -> MemResult<&mut [T]> {
        RegisteredMemoryRegion::<T>::as_mut_slice(self)
    }
    pub fn as_ptr(&self) -> MemResult<*const T> {
        RegisteredMemoryRegion::<T>::as_ptr(self)
    }
    pub fn as_mut_ptr(&self) -> MemResult<*mut T> {
        RegisteredMemoryRegion::<T>::as_mut_ptr(self)
    }
}
//account for subregion stuff
impl<T: Arraydist> RegisteredMemoryRegion<T> for SharedMemoryRegion<T> {
    fn len(&self) -> usize {
        self.sub_region_size
    }
    fn addr(&self) -> MemResult<usize> {
        if let Ok(addr) = self.mr.addr() {
            Ok(addr + self.sub_region_offset * std::mem::size_of::<T>())
        } else {
            Err(MemNotLocalError {})
        }
    }
    fn at(&self, index: usize) -> MemResult<&T> {
        self.mr.casted_at::<T>(index)
    }
    fn as_slice(&self) -> MemResult<&[T]> {
        if let Ok(slice) = self.mr.as_casted_slice::<T>() {
            Ok(&slice[self.sub_region_offset..(self.sub_region_offset + self.sub_region_size)])
        } else {
            Err(MemNotLocalError {})
        }
    }
    unsafe fn as_mut_slice(&self) -> MemResult<&mut [T]> {
        if let Ok(slice) = self.mr.as_casted_mut_slice::<T>() {
            Ok(&mut slice[self.sub_region_offset..(self.sub_region_offset + self.sub_region_size)])
        } else {
            Err(MemNotLocalError {})
        }
    }
    fn as_ptr(&self) -> MemResult<*const T> {
        if let Ok(addr) = self.addr() {
            Ok(addr as *const T)
        } else {
            Err(MemNotLocalError {})
        }
    }
    fn as_mut_ptr(&self) -> MemResult<*mut T> {
        if let Ok(addr) = self.addr() {
            Ok(addr as *mut T)
        } else {
            Err(MemNotLocalError {})
        }
    }
}

impl<T: Arraydist> MemRegionId for SharedMemoryRegion<T> {
    fn id(&self) -> usize {
        self.mr.id()
    }
}

impl<T: Arraydist> SubRegion<T> for SharedMemoryRegion<T> {
    fn sub_region<R: std::ops::RangeBounds<usize>>(&self, range: R) -> LamellarMemoryRegion<T> {
        let start = match range.start_bound() {
            //inclusive
            Bound::Included(idx) => *idx,
            Bound::Excluded(idx) => *idx + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            //exclusive
            Bound::Included(idx) => *idx + 1,
            Bound::Excluded(idx) => *idx,
            Bound::Unbounded => self.sub_region_size,
        };
        if end > self.sub_region_size {
            panic!(
                "subregion range ({:?}-{:?}) exceeds size of memregion {:?}",
                start, end, self.sub_region_size
            );
        }
        // println!("shared subregion: {:?} {:?} {:?}",start,end,(end-start));
        SharedMemoryRegion {
            mr: self.mr.clone(),
            sub_region_offset: self.sub_region_offset + start,
            sub_region_size: (end - start),
            phantom: PhantomData,
        }
        .into()
    }
}

impl<T: Arraydist> AsBase for SharedMemoryRegion<T> {
    unsafe fn to_base<B: Arraydist>(self) -> LamellarMemoryRegion<B> {
        let u8_offset = self.sub_region_offset * std::mem::size_of::<T>();
        let u8_size = self.sub_region_size * std::mem::size_of::<T>();
        SharedMemoryRegion {
            mr: self.mr.clone(),
            sub_region_offset: u8_offset / std::mem::size_of::<B>(),
            sub_region_size: u8_size / std::mem::size_of::<B>(),
            phantom: PhantomData,
        }
        .into()
    }
}

//#[prof]
impl<T: Arraydist> MemoryRegionRDMA<T> for SharedMemoryRegion<T> {
    unsafe fn put<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        self.mr.put(pe, self.sub_region_offset + index, data);
    }
    fn iput<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        self.mr.iput(pe, self.sub_region_offset + index, data);
    }
    unsafe fn put_all<U: Into<LamellarMemoryRegion<T>>>(&self, index: usize, data: U) {
        self.mr.put_all(self.sub_region_offset + index, data);
    }
    unsafe fn get<U: Into<LamellarMemoryRegion<T>>>(&self, pe: usize, index: usize, data: U) {
        self.mr.get(pe, self.sub_region_offset + index, data);
    }
}

impl<T: Arraydist> RTMemoryRegionRDMA<T> for SharedMemoryRegion<T> {
    unsafe fn put_slice(&self, pe: usize, index: usize, data: &[T]) {
        self.mr.put_slice(pe, self.sub_region_offset + index, data)
    }
}

impl<T: Arraydist> std::fmt::Debug for SharedMemoryRegion<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] shared mem region:  {:?} ", self.mr.pe, self.mr,)
    }
}

impl<T: Arraydist> From<&SharedMemoryRegion<T>> for LamellarArrayInput<T> {
    fn from(smr: &SharedMemoryRegion<T>) -> Self {
        LamellarArrayInput::SharedMemRegion(smr.clone())
    }
}

impl<T: Arraydist> MyFrom<&SharedMemoryRegion<T>> for LamellarArrayInput<T> {
    fn my_from(smr: &SharedMemoryRegion<T>, _team: &Arc<LamellarTeamRT>) -> Self {
        LamellarArrayInput::SharedMemRegion(smr.clone())
    }
}

// //#[prof]
// impl<T: Dist> Drop for SharedMemoryRegion<T> {
//     fn drop(&mut self) {
//         println!("dropping shared memory region");
//     }
// }
