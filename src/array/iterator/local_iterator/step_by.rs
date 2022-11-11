use crate::array::iterator::local_iterator::*;

//ignores the first n elements of iterator I per pe (this implys that n * num_pes elements are ignored in total)
#[derive(Clone, Debug)]
pub struct StepBy<I> {
    iter: I,
    step_size: usize,
    add_one: usize, //if we dont align perfectly we will need to add 1 to our iteration index calculation
}

impl<I> StepBy<I>
where
    I: IndexedLocalIterator,
{
    pub(crate) fn new(iter: I, step_size: usize, add_one: usize) -> StepBy<I> {
        StepBy { iter, step_size, add_one }
    }
}

impl<I> LocalIterator for StepBy<I>
where
    I: IndexedLocalIterator,
{
    type Item = <I as LocalIterator>::Item;
    type Array = <I as LocalIterator>::Array;
    fn init(&self, in_start_i: usize, cnt: usize) -> StepBy<I> {
        let mut iter = self.iter.init(in_start_i * self.step_size,  cnt * self.step_size);
        let mut offset_index = 0;

        // make sure we start from a valid step interval element
        if let Some(mut iterator_index) = iter.iterator_index(in_start_i* self.step_size+offset_index){
            // println!("{:?} StepBy init {in_start_i} {iterator_index}",std::thread::current().id());
            while iterator_index % self.step_size != 0 { 
                // println!("{:?} StepBy init {in_start_i} {} {} {} {iterator_index}",std::thread::current().id(),in_start_i* self.step_size+offset_index,cnt * self.step_size,self.step_size);
                offset_index +=1;
                match iter.iterator_index(in_start_i* self.step_size+offset_index) {
                    Some(i) => iterator_index = i,
                    None => {
                        iter.advance_index(cnt);
                        let val = StepBy::new(iter, self.step_size,1);
                        // println!("{:?} StepBy nothing init {} {} {} ",std::thread::current().id(),in_start_i* self.step_size+offset_index,cnt * self.step_size,self.step_size);
                        return val;
                    }, // step size larger than number of elements
                }
            }
            iter.advance_index(offset_index);
            let val = StepBy::new(
                iter,
                self.step_size,
                (offset_index > 0) as usize
            );

            // println!("{:?} StepBy init {} {} {} ",std::thread::current().id(),in_start_i* self.step_size+offset_index,cnt * self.step_size,self.step_size);
            val
        }
        else { // nothing to iterate so set len to 0
            iter.advance_index(cnt);
            let val = StepBy::new(
                iter,
                self.step_size,
                0,
            );
            // println!("{:?} StepBy nothing init {} {} {} ",std::thread::current().id(),in_start_i * self.step_size,cnt * self.step_size,self.step_size);
            val
        }
    }
    fn array(&self) -> Self::Array {
        self.iter.array()
    }

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.iter.next();
        self.iter.advance_index(self.step_size - 1); //-1 cause iter.next() already advanced by 1
        res
    }

    fn elems(&self, in_elems: usize) -> usize {
        let in_elems = self.iter.elems(in_elems);
        (in_elems as f32 / self.step_size as f32).ceil() as usize
    }

    fn subarray_index(&self, index: usize) -> Option<usize> {
        let g_index = self.iter.subarray_index(index * self.step_size)? / self.step_size; 
        Some(g_index)
    }
    
    fn advance_index(&mut self, count: usize) {
        self.iter.advance_index(count);
    }
}

impl<I> IndexedLocalIterator for StepBy<I>
where
    I: IndexedLocalIterator,
{
    fn iterator_index(&self, index: usize) -> Option<usize> {
        if let Some(mut g_index) = self.iter.iterator_index(index * self.step_size){
            g_index= g_index/ self.step_size + self.add_one; 
            // println!("{:?} \t StepBy iterator index {index} {g_index}",std::thread::current().id());
            Some(g_index)
        }
        else {
            // println!("{:?} \t StepBy iterator index {index} None",std::thread::current().id());
            None
        } 
    }
}
