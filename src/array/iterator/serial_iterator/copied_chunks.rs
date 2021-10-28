use crate::memregion::Dist;
use crate::LamellarArray;
use crate::LocalMemoryRegion;



pub struct LamellarArrayChunksIter<
T: Dist + serde::ser::Serialize + serde::de::DeserializeOwned + 'static,
> {
array: LamellarArray<T>,
mem_region: LocalMemoryRegion<T>,
index: usize,
chunk_size: usize,
}

impl<T: Dist + serde::ser::Serialize + serde::de::DeserializeOwned + 'static>
LamellarArrayChunksIter<T>
{
pub(crate) fn new(array: LamellarArray<T>, chunk_size: usize) -> Self {
    let mem_region = array.team().alloc_local_mem_region(chunk_size);
    let chunks = LamellarArrayChunksIter {
        array: array,
        mem_region: mem_region.clone(),
        index: 0,
        chunk_size: chunk_size,
    };
    chunks.fill_buffer(0, &mem_region);
    chunks
}

fn fill_buffer(&self, val: u8, buf: &LocalMemoryRegion<T>) {
    let buf_u8 = buf.clone().to_base::<u8>();
    let buf_slice = unsafe { buf_u8.as_mut_slice().unwrap() };
    for i in 0..buf_slice.len() {
        buf_slice[i] = val;
    }
    self.array.get(self.index, buf);
}

fn spin_for_valid(&self, buf: &LocalMemoryRegion<T>) {
    let buf_0_temp = self.mem_region.clone().to_base::<u8>();
    let buf_0 = buf_0_temp.as_slice().unwrap();
    let buf_1_temp = buf.clone().to_base::<u8>();
    let buf_1 = buf_1_temp.as_slice().unwrap();
    for i in 0..buf_1.len() {
        while buf_0[i] != buf_1[i] {
            std::thread::yield_now();
        }
    }
}
}

impl<T: Dist + serde::ser::Serialize + serde::de::DeserializeOwned + 'static> Iterator
for LamellarArrayChunksIter<T>
{
type Item = LocalMemoryRegion<T>;
fn next(&mut self) -> Option<Self::Item> {
    let res = if self.index < self.array.len() {
        let size = std::cmp::min(self.chunk_size, self.array.len() - self.index);
        let mem_region = self.array.team().alloc_local_mem_region(size);
        self.fill_buffer(1, &mem_region);
        self.spin_for_valid(&mem_region);
        self.index += size;
        if self.index < self.array.len() {
            let size = std::cmp::min(self.chunk_size, self.array.len() - self.index);
            self.fill_buffer(0, &self.mem_region.sub_region(..size));
        }
        Some(mem_region)
    } else {
        None
    };
    res
}
}