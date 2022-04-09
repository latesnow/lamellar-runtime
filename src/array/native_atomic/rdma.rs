use crate::array::native_atomic::*;
use crate::array::private::ArrayExecAm;
use crate::array::LamellarWrite;
use crate::array::*;
use crate::memregion::{AsBase, Dist, RTMemoryRegionRDMA, RegisteredMemoryRegion};

impl<T: Dist + 'static> NativeAtomicArray<T> {
    pub fn get<U: MyInto<LamellarArrayInput<T>> + LamellarWrite>(
        &self,
        index: usize,
        buf: U,
    ) -> Box<dyn LamellarArrayRequest<Output = ()> + Send + Sync> {
        let req = self.exec_am_local(InitGetAm {
            array: self.clone(),
            index: index,
            buf: buf.my_into(&self.array.team()),
        });
        Box::new(ArrayRdmaHandle { reqs: vec![req] })
    }

    pub fn at(&self, index: usize) -> Box<dyn LamellarArrayRequest<Output = T> + Send + Sync> {
        let buf: LocalMemoryRegion<T> = self.array.team().alloc_local_mem_region(1);
        let req = self.exec_am_local(InitGetAm {
            array: self.clone(),
            index: index,
            buf: buf.clone().my_into(&self.array.team()),
        });
        Box::new(ArrayRdmaAtHandle {
            reqs: vec![req],
            buf: buf,
        })
    }

    // pub fn iput<U: MyInto<LamellarArrayInput<T>> + LamellarRead>(&self, index: usize, buf: U) {
    //     self.exec_am_local(InitPutAm {
    //         array: self.clone(),
    //         index: index,
    //         buf: buf.my_into(&self.array.team()),
    //     })
    //     .get();
    // }

    pub fn put<U: MyInto<LamellarArrayInput<T>> + LamellarRead>(
        &self,
        index: usize,
        buf: U,
    ) -> Box<dyn LamellarArrayRequest<Output = ()> + Send + Sync> {
        let req = self.exec_am_local(InitPutAm {
            array: self.clone(),
            index: index,
            buf: buf.my_into(&self.array.team()),
        });
        Box::new(ArrayRdmaHandle { reqs: vec![req] })
    }
}

impl<T: Dist + 'static> LamellarArrayGet<T> for NativeAtomicArray<T> {
    // fn iget<U: MyInto<LamellarArrayInput<T>> + LamellarWrite>(&self, index: usize, buf: U) {
    //     self.iget(index, buf)
    // }
    fn get<U: MyInto<LamellarArrayInput<T>> + LamellarWrite>(
        &self,
        index: usize,
        buf: U,
    ) -> Box<dyn LamellarArrayRequest<Output = ()> + Send + Sync> {
        self.array.get(index, buf)
    }
    fn at(&self, index: usize) -> Box<dyn LamellarArrayRequest<Output = T> + Send + Sync> {
        self.array.at(index)
    }
}

impl<T: Dist> LamellarArrayPut<T> for NativeAtomicArray<T> {
    fn put<U: MyInto<LamellarArrayInput<T>> + LamellarRead>(
        &self,
        index: usize,
        buf: U,
    ) -> Box<dyn LamellarArrayRequest<Output = ()> + Send + Sync> {
        self.put(index, buf)
    }
}

#[lamellar_impl::AmLocalDataRT]
struct InitGetAm<T: Dist> {
    array: NativeAtomicArray<T>, //inner of the indices we need to place data into
    index: usize,                //relative to inner
    buf: LamellarArrayInput<T>,
}

#[lamellar_impl::rt_am_local]
impl<T: Dist + 'static> LamellarAm for InitGetAm<T> {
    fn exec(self) {
        // let buf = self.buf.into();
        // let u8_index = self.index * std::mem::size_of::<T>();
        // let u8_len = self.buf.len() * std::mem::size_of::<T>();
        // println!("in native InitGetAm ");//{:?} {:?}",u8_index,u8_index + u8_len);
        let mut reqs = vec![];
        for pe in self
            .array
            .array
            .pes_for_range(self.index, self.buf.len())
            .into_iter()
        {
            // println!("pe {:?}",pe);
            let remote_am = NativeAtomicRemoteGetAm {
                array: self.array.clone().into(),
                start_index: self.index,
                len: self.buf.len(),
            };
            reqs.push(self.array.exec_am_pe(pe, remote_am).into_future());
        }
        unsafe {
            match self.array.array.inner.distribution {
                Distribution::Block => {
                    let u8_buf = self.buf.clone().to_base::<u8>();
                    let mut cur_index = 0;
                    for req in reqs.drain(..) {
                        let data = req.await.unwrap();
                        // println!("data recv {:?}",data.len());
                        u8_buf.put_slice(lamellar::current_pe, cur_index, &data);
                        cur_index += data.len();
                    }
                }
                Distribution::Cyclic => {
                    let buf_slice = self.buf.as_mut_slice().unwrap();
                    let num_pes = reqs.len();
                    for (start_index, req) in reqs.drain(..).enumerate() {
                        let data = req.await.unwrap();
                        let data_t_ptr = data.as_ptr() as *const T;
                        let data_t_len = if data.len() % std::mem::size_of::<T>() == 0 {
                            data.len() / std::mem::size_of::<T>()
                        } else {
                            panic!("memory align error");
                        };
                        let data_t_slice = std::slice::from_raw_parts(data_t_ptr, data_t_len);
                        for (i, val) in data_t_slice.iter().enumerate() {
                            buf_slice[start_index + i * num_pes] = *val;
                        }
                    }
                }
            }
        }
    }
}

#[lamellar_impl::AmDataRT]
struct NativeAtomicRemoteGetAm {
    array: NativeAtomicByteArray, //inner of the indices we need to place data into
    start_index: usize,
    len: usize,
}

#[lamellar_impl::rt_am]
impl LamellarAm for NativeAtomicRemoteGetAm {
    //we cant directly do a put from the array in to the data buf
    //because we need to guarantee the put operation is atomic (maybe iput would work?)
    fn exec(self) -> Vec<u8> {
        // println!("in nativeAtomic remotegetam {:?} {:?}",self.start_index,self.len);
        unsafe {
            match self
                .array
                .array
                .local_elements_for_range(self.start_index, self.len)
            {
                Some((elems, _indices)) => {
                    let mut data = elems.to_vec();
                    let src_ptr = elems.as_mut_ptr();
                    let dst_ptr = data.as_mut_ptr();
                    for offset in (0..data.len()).step_by(self.array.orig_t.size()) {
                        self.array.orig_t.load(
                            src_ptr.offset(offset as isize),
                            dst_ptr.offset(offset as isize),
                        );
                    }
                    data
                }
                None => vec![],
            }
        }
    }
}

#[lamellar_impl::AmLocalDataRT]
struct InitPutAm<T: Dist> {
    array: NativeAtomicArray<T>, //inner of the indices we need to place data into
    index: usize,                //relative to inner
    buf: LamellarArrayInput<T>,
}

#[lamellar_impl::rt_am_local]
impl<T: Dist + 'static> LamellarAm for InitPutAm<T> {
    fn exec(self) {
        // let u8_index = self.index * std::mem::size_of::<T>();
        // let u8_len = self.buf.len() * std::mem::size_of::<T>();

        unsafe {
            let u8_buf = self.buf.clone().to_base::<u8>();
            let mut reqs = vec![];
            match self.array.array.inner.distribution {
                Distribution::Block => {
                    let mut cur_index = 0;
                    for pe in self
                        .array
                        .array
                        .pes_for_range(self.index, self.buf.len())
                        .into_iter()
                    {
                        if let Some(len) = self.array.array.num_elements_on_pe_for_range(
                            pe,
                            self.index,
                            self.buf.len(),
                        ) {
                            let u8_buf_len = len * std::mem::size_of::<T>();
                            // println!("pe {:?} index: {:?} len {:?} buflen {:?} putting {:?}",pe,self.index,len, self.buf.len(),&u8_buf.as_slice().unwrap()[cur_index..(cur_index+u8_buf_len)]);
                            let remote_am = NativeAtomicRemotePutAm {
                                array: self.array.clone().into(), //inner of the indices we need to place data into
                                start_index: self.index,
                                len: self.buf.len(),
                                data: u8_buf.as_slice().unwrap()
                                    [cur_index..(cur_index + u8_buf_len)]
                                    .to_vec(),
                            };
                            reqs.push(self.array.exec_am_pe(pe, remote_am).into_future());
                            cur_index += u8_buf_len;
                        } else {
                            panic!("this should not be possible");
                        }
                    }
                }
                Distribution::Cyclic => {
                    let num_pes = ArrayExecAm::team(&self.array).num_pes();
                    let mut pe_u8_vecs: HashMap<usize, Vec<u8>> = HashMap::new();
                    let mut pe_t_slices: HashMap<usize, &mut [T]> = HashMap::new();
                    let buf_slice = self.buf.as_slice().unwrap();
                    for pe in self
                        .array
                        .array
                        .pes_for_range(self.index, self.buf.len())
                        .into_iter()
                    {
                        if let Some(len) = self.array.array.num_elements_on_pe_for_range(
                            pe,
                            self.index,
                            self.buf.len(),
                        ) {
                            // println!("len {:?} buf len {:?} ",len,self.buf.len());
                            let mut u8_vec = vec![0u8; len * std::mem::size_of::<T>()];
                            let t_slice =
                                std::slice::from_raw_parts_mut(u8_vec.as_mut_ptr() as *mut T, len);
                            pe_u8_vecs.insert(pe, u8_vec);
                            pe_t_slices.insert(pe, t_slice);
                        }
                    }
                    for (buf_index, index) in
                        (self.index..(self.index + self.buf.len())).enumerate()
                    {
                        let pe = self.array.array.pe_for_dist_index(index).unwrap() % num_pes;
                        // println!("pe {:?} tslice index {:?} buf_index {:?}",pe,buf_index/num_pes,buf_index);
                        pe_t_slices.get_mut(&pe).unwrap()[buf_index / num_pes] =
                            buf_slice[buf_index];
                    }
                    for (pe, vec) in pe_u8_vecs.drain() {
                        // println!("pe {:?} vec {:?}",pe,vec);
                        let remote_am = NativeAtomicRemotePutAm {
                            array: self.array.clone().into(), //inner of the indices we need to place data into
                            start_index: self.index,
                            len: self.buf.len(),
                            data: vec,
                        };
                        reqs.push(self.array.exec_am_pe(pe, remote_am).into_future());
                    }
                }
            }
            for req in reqs.drain(..) {
                req.await;
            }
            // println!("done local put");
        }
    }
}

#[lamellar_impl::AmDataRT]
struct NativeAtomicRemotePutAm {
    array: NativeAtomicByteArray, //inner of the indices we need to place data into
    start_index: usize,
    len: usize,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

#[lamellar_impl::rt_am]
impl LamellarAm for NativeAtomicRemotePutAm {
    fn exec(self) {
        // println!("in remote put {:?} {:?} {:?}",self.start_index,self.len,self.data);
        // let _lock = self.array.lock.write();
        unsafe {
            match self
                .array
                .array
                .local_elements_for_range(self.start_index, self.len)
            {
                Some((elems, _indices)) => {
                    // println!("elems: {:?}",elems);
                    let src_ptr = self.data.as_ptr();
                    let dst_ptr = elems.as_mut_ptr();
                    for offset in (0..elems.len()).step_by(self.array.orig_t.size()) {
                        self.array.orig_t.store(
                            src_ptr.offset(offset as isize),
                            dst_ptr.offset(offset as isize),
                        );
                    }
                    // std::ptr::copy_nonoverlapping(
                    //     self.data.as_ptr(),
                    //     elems.as_mut_ptr(),
                    //     elems.len(),
                    // )
                }
                None => {}
            }
        }
        // println!("done remote put");
    }
}
