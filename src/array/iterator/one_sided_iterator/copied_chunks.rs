use crate::array::iterator::one_sided_iterator::*;
use crate::array::LamellarArrayRequest;
// use crate::LamellarArray;
use crate::memregion::OneSidedMemoryRegion;
use pin_project::pin_project;

use async_trait::async_trait;
// use futures::Future;
#[pin_project]
pub struct CopiedChunks<I>
where
    I: OneSidedIterator + Send,
{
    #[pin]
    iter: I,
    // array: LamellarArray<I::ElemType>,
    // mem_region: OneSidedMemoryRegion<I::ElemType>,
    index: usize,
    chunk_size: usize,
}

impl<I> CopiedChunks<I>
where
    I: OneSidedIterator + Send,
{
    pub(crate) fn new(iter: I, chunk_size: usize) -> CopiedChunks<I> {
        // let array = iter.array().clone(); //.to_base::<u8>();
        // println!("len: {:?}",array.len());
        // let mem_region = iter.array().team().alloc_one_sided_mem_region(chunk_size);//*iter.array().size_of_elem());
        let chunks = CopiedChunks {
            iter,
            // array,
            // mem_region: mem_region.clone(),
            index: 0,
            chunk_size,
        };
        // chunks.fill_buffer(0,&mem_region);
        chunks
    }

    fn get_buffer(&self, size: usize) -> OneSidedMemoryRegion<<I as OneSidedIterator>::ElemType> {
        let mem_region: OneSidedMemoryRegion<<I as OneSidedIterator>::ElemType> =
            self.array().team().alloc_one_sided_mem_region(size);
        self.array().internal_get(self.index, &mem_region).wait();
        mem_region
    }

    // fn get_buffer_async(
    //     self: &Pin<&mut Self>,
    //     size: usize,
    // ) -> Pin<Box<dyn Future<Output = OneSidedMemoryRegion<<I as OneSidedIterator>::ElemType>> + Send>>
    // {
    //     // let this = self.project();
    //     let array = self.iter.array();
    //     let mem_region: OneSidedMemoryRegion<<I as OneSidedIterator>::ElemType> =
    //         array.team().alloc_one_sided_mem_region(size);
    //     let index = self.index;
    //     let req = array.internal_get(index, &mem_region).into_future();
    //     Box::pin(async {
    //         req.await;
    //         mem_region
    //     })
    // }
}

#[async_trait]
// impl<I> SerialAsyncIterator for CopiedChunks<I>
// where
//     I: OneSidedIterator + SerialAsyncIterator,
// {
//     type ElemType = <I as SerialAsyncIterator>::ElemType;
//     type Item = OneSidedMemoryRegion<<I as SerialAsyncIterator>::ElemType>;
//     type Array = <I as SerialAsyncIterator>::Array;
//     async fn async_next(self: Pin<&mut Self>) -> Option<Self::Item> {
//         // println!("{:?} {:?}",self.index,self.array.len()/std::mem::size_of::<<Self as OneSidedIterator>::ElemType>());
//         let array = self.array();
//         if self.index < array.len() {
//             let size = std::cmp::min(self.chunk_size, array.len() - self.index);

//             let mem_region = self.get_buffer_async(size).await;
//             self.index += size;
//             Some(mem_region)
//         } else {
//             None
//         }
//     }
// }
impl<I> OneSidedIterator for CopiedChunks<I>
where
    I: OneSidedIterator + Send,
{
    type ElemType = I::ElemType;
    type Item = OneSidedMemoryRegion<I::ElemType>;
    type Array = I::Array;
    fn next(&mut self) -> Option<Self::Item> {
        // println!("{:?} {:?}",self.index,self.array.len()/std::mem::size_of::<<Self as OneSidedIterator>::ElemType>());
        let array = self.array();
        if self.index < array.len() {
            let size = std::cmp::min(self.chunk_size, array.len() - self.index);

            let mem_region = self.get_buffer(size);
            self.index += size;
            Some(mem_region)
        } else {
            None
        }
    }
    // async fn async_next(self: Pin<&mut Self>) -> Option<Self::Item> {
    //     // println!("async_next copied_chunks");
    //     // let mut this = self.project();
    //     // println!("{:?} {:?}",self.index,self.array.len()/std::mem::size_of::<<Self as OneSidedIterator>::ElemType>());
    //     let array = self.iter.array();
    //     if self.index < array.len() {
    //         let size = std::cmp::min(self.chunk_size, array.len() - self.index);

    //         let mem_region = self.get_buffer_async(size);
    //         *self.project().index += size;
    //         Some(mem_region.await)
    //     } else {
    //         None
    //     }
    // }
    fn advance_index(&mut self, count: usize) {
        // println!("advance_index {:?} {:?} {:?} {:?}",self.index, count, count*self.chunk_size,self.array.len());
        self.index += count * self.chunk_size;
        // if self.index < self.array.len(){
        //     let size = std::cmp::min(self.chunk_size, self.array.len() - self.index);
        //     self.fill_buffer(0, &self.mem_region.sub_region(..size));
        // }
    }
    // async fn async_advance_index(mut self: Pin<&mut Self>, count: usize) {
    //     let this = self.project();
    //     *this.index += count * *this.chunk_size;
    // }
    fn array(&self) -> Self::Array {
        self.iter.array()
    }
    fn item_size(&self) -> usize {
        self.chunk_size * std::mem::size_of::<I::ElemType>()
    }
    fn buffered_next(
        &mut self,
        mem_region: OneSidedMemoryRegion<u8>,
    ) -> Option<Box<dyn LamellarArrayRequest<Output = ()>>> {
        let array = self.array();
        if self.index < array.len() {
            let mem_reg_t = unsafe { mem_region.to_base::<I::ElemType>() };
            let req = array.internal_get(self.index, &mem_reg_t);
            self.index += mem_reg_t.len();
            Some(req)
        } else {
            None
        }
    }
    // async fn async_buffered_next(
    //     mut self: Pin<&mut Self>,
    //     mem_region: OneSidedMemoryRegion<u8>,
    // ) -> Option<Box<dyn LamellarArrayRequest<Output = ()>>> {
    //     let array = self.array();
    //     if self.index < array.len() {
    //         let mem_reg_t = mem_region.to_base::<I::ElemType>();
    //         let req = array.internal_get(self.index, &mem_reg_t);
    //         let this = self.project();
    //         *this.index += mem_reg_t.len();
    //         Some(req)
    //     } else {
    //         None
    //     }
    // }
    fn from_mem_region(&self, mem_region: OneSidedMemoryRegion<u8>) -> Option<Self::Item> {
        let mem_reg_t = unsafe { mem_region.to_base::<I::ElemType>() };
        Some(mem_reg_t)
    }
}

// impl<I> Iterator for CopiedChunks<I>
// where
//     I: OneSidedIterator + Iterator
// {
//     type Item = OneSidedMemoryRegion<I::ElemType>;
//     fn next(&mut self) -> Option<Self::Item> {
//         <Self as OneSidedIterator>::next(self)
//     }
// }

// use futures::task::{Context, Poll};
// use futures::Stream;
// use std::pin::Pin;

// impl<I> Stream for CopiedChunks<I>
// where
//     I: OneSidedIterator + Stream + Unpin
// {
//     type Item = OneSidedMemoryRegion<I::ElemType>;
//     fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
//         // println!("{:?} {:?}",self.index,self.array.len()/std::mem::size_of::<<Self as OneSidedIterator>::ElemType>());
//         println!("async getting {:?} {:?}",self.index,self.chunk_size);
//         if self.index < self.array.len(){
//             let size = std::cmp::min(self.chunk_size, self.array.len() - self.index);
//             // self.fill_buffer(0, &self.mem_region.sub_region(..size));
//             let mem_region: OneSidedMemoryRegion<I::ElemType> = self.array.team().alloc_one_sided_mem_region(size);
//             self.fill_buffer(101010101, &mem_region);
//             if self.check_for_valid(101010101,&mem_region){
//                 self.index += size;
//                 Poll::Ready(Some(mem_region))
//             }
//             else{
//                 Poll::Pending
//             }
//         }
//         else{
//             Poll::Ready(None)
//         }
//     }
// }