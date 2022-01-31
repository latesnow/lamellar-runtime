use crate::active_messaging::*;
use crate::array::collective_atomic::*;
use crate::array::*;
use crate::lamellar_request::LamellarRequest;
// use crate::memregion::Dist;
use std::any::TypeId;
use std::collections::HashMap;

type OpFn = fn(*const u8, CollectiveAtomicByteArray, usize) -> LamellarArcAm;

lazy_static! {
    static ref OPS: HashMap<(ArrayOpCmd, TypeId), OpFn> = {
        let mut map = HashMap::new();
        for op in crate::inventory::iter::<CollectiveAtomicArrayOp> {
            map.insert(op.id.clone(), op.op);
        }
        map
    };
}

pub struct CollectiveAtomicArrayOp {
    pub id: (ArrayOpCmd, TypeId),
    pub op: OpFn,
}

crate::inventory::collect!(CollectiveAtomicArrayOp);

impl<T: AmDist + Dist + 'static> CollectiveAtomicArray<T> {
    fn initiate_op(
        &self,
        index: usize,
        val: T,
        local_index: usize,
        op: ArrayOpCmd,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        // println!("initiate_op for CollectiveAtomicArray<T> ");
        if let Some(func) = OPS.get(&(op, TypeId::of::<T>())) {
            let array: CollectiveAtomicByteArray = self.clone().into();
            let pe = self.pe_for_dist_index(index).expect("index out of bounds");
            let am = func(&val as *const T as *const u8, array, local_index);
            // Some(self.inner.team.exec_arc_am_pe(
            //     pe,
            //     am,
            //     Some(self.inner.array_counters.clone()),
            // ))
            Some(self.array.dist_op(pe, am))
        } else {
            let name = std::any::type_name::<T>().split("::").last().unwrap();
            panic!("the type {:?} has not been registered! this typically means you need to derive \"ArithmeticOps\" for the type . e.g. 
            #[derive(lamellar::ArithmeticOps)]
            struct {:?}{{
                ....
            }}
            note this also requires the type to impl Serialize + Deserialize, you can manually derive these, or use the lamellar::AmData attribute proc macro, e.g.
            #[lamellar::AMData(ArithmeticOps, any other traits you derive)]
            struct {:?}{{
                ....
            }}",name,name,name);
        }
    }

    fn initiate_fetch_op(
        &self,
        index: usize,
        val: T,
        local_index: usize,
        op: ArrayOpCmd,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        // println!("initiate_op for CollectiveAtomicArray<T> ");
        if let Some(func) = OPS.get(&(op, TypeId::of::<T>())) {
            let array: CollectiveAtomicByteArray = self.clone().into();
            let pe = self.pe_for_dist_index(index).expect("index out of bounds");
            let am = func(&val as *const T as *const u8, array, local_index);
            // self.inner.team.exec_arc_am_pe(
            //     pe,
            //     am,
            //     Some(self.inner.array_counters.clone()),
            // )
            self.array.dist_fetch_op(pe, am)
        } else {
            let name = std::any::type_name::<T>().split("::").last().unwrap();
            panic!("the type {:?} has not been registered! this typically means you need to derive \"ArithmeticOps\" for the type . e.g. 
            #[derive(lamellar::ArithmeticOps)]
            struct {:?}{{
                ....
            }}
            note this also requires the type to impl Serialize + Deserialize, you can manually derive these, or use the lamellar::AmData attribute proc macro, e.g.
            #[lamellar::AMData(ArithmeticOps, any other traits you derive)]
            struct {:?}{{
                ....
            }}",name,name,name);
        }
    }

    pub fn store(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_store(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Store)
        }
    }

    pub fn load(&self, index: usize) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        let dummy_val = self.array.dummy_val(); //we dont actually do anything with this except satisfy apis;
        if pe == self.my_pe() {
            let val = self.local_load(local_index, dummy_val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, dummy_val, local_index, ArrayOpCmd::Load)
        }
    }

    pub fn swap(&self, index: usize, val: T) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_swap(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::Swap)
        }
    }
}

impl<T: ElementArithmeticOps + 'static> ArithmeticOps<T> for CollectiveAtomicArray<T> {
    fn add(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
                                                                             // println!("index {:?} pe {:?} local_index {:?}",index,pe,local_index);
        if pe == self.my_pe() {
            self.local_add(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Add)
        }
    }
    fn fetch_add(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_add(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchAdd)
        }
    }
    fn sub(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_sub(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Sub)
        }
    }
    fn fetch_sub(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_sub(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchSub)
        }
    }
    fn mul(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_mul(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Mul)
        }
    }
    fn fetch_mul(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_mul(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchMul)
        }
    }
    fn div(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_div(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Div)
        }
    }
    fn fetch_div(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_div(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchDiv)
        }
    }
}

impl<T: ElementBitWiseOps + 'static> BitWiseOps<T> for CollectiveAtomicArray<T> {
    fn bit_and(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_bit_and(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::And)
        }
    }
    fn fetch_bit_and(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_bit_and(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchAnd)
        }
    }

    fn bit_or(
        &self,
        index: usize,
        val: T,
    ) -> Option<Box<dyn LamellarRequest<Output = ()> + Send + Sync>> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            self.local_bit_or(local_index, val);
            None
        } else {
            self.initiate_op(index, val, local_index, ArrayOpCmd::Or)
        }
    }
    fn fetch_bit_or(
        &self,
        index: usize,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = T> + Send + Sync> {
        let pe = self.pe_for_dist_index(index).expect("index out of bounds");
        let local_index = self.pe_offset_for_dist_index(pe, index).unwrap(); //calculated pe above
        if pe == self.my_pe() {
            let val = self.local_fetch_bit_or(local_index, val);
            Box::new(LocalOpResult { val })
        } else {
            self.initiate_fetch_op(index, val, local_index, ArrayOpCmd::FetchOr)
        }
    }
}

// impl<T: Dist + std::ops::AddAssign> CollectiveAtomicArray<T> {
impl<T: ElementArithmeticOps> LocalArithmeticOps<T> for CollectiveAtomicArray<T> {
    fn local_fetch_add(&self, index: usize, val: T) -> T {
        // println!("local_add LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        let _lock = self.lock.write();
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] += val;
            orig
        }
    }
    fn local_fetch_sub(&self, index: usize, val: T) -> T {
        // println!("local_sub LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        let _lock = self.lock.write();
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] -= val;
            orig
        }
    }
    fn local_fetch_mul(&self, index: usize, val: T) -> T {
        // println!("local_sub LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        let _lock = self.lock.write();
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] *= val;
            orig
        }
    }
    fn local_fetch_div(&self, index: usize, val: T) -> T {
        // println!("local_sub LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        let _lock = self.lock.write();
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] /= val;
            // println!("div i: {:?} {:?} {:?} {:?}",index,orig,val,self.local_as_mut_slice()[index]);
            orig
        }
    }
}
impl<T: ElementBitWiseOps> LocalBitWiseOps<T> for CollectiveAtomicArray<T> {
    fn local_fetch_bit_and(&self, index: usize, val: T) -> T {
        let _lock = self.lock.write();
        // println!("local_sub LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] &= val;
            // println!("and i: {:?} {:?} {:?} {:?}",index,orig,val,self.local_as_mut_slice()[index]);
            orig
        }
    }
    fn local_fetch_bit_or(&self, index: usize, val: T) -> T {
        let _lock = self.lock.write();
        // println!("local_sub LocalArithmeticOps<T> for CollectiveAtomicArray<T> ");
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] |= val;
            orig
        }
    }
}
impl<T: ElementOps> LocalAtomicOps<T> for CollectiveAtomicArray<T> {
    fn local_load(&self, index: usize, _val: T) -> T {
        let _lock = self.lock.read();
        unsafe { self.local_as_mut_slice()[index] }
    }

    fn local_store(&self, index: usize, val: T) {
        let _lock = self.lock.write();
        unsafe {
            self.local_as_mut_slice()[index] = val;
        }
    }

    fn local_swap(&self, index: usize, val: T) -> T {
        let _lock = self.lock.write();
        unsafe {
            let orig = self.local_as_mut_slice()[index];
            self.local_as_mut_slice()[index] = val;
            orig
        }
    }
}
// }

#[macro_export]
macro_rules! CollectiveAtomicArray_create_ops {
    ($a:ty, $name:ident) => {
        paste::paste!{
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Add,[<$name dist_add>],[<$name local_add>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchAdd,[<$name dist_fetch_add>],[<$name local_add>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Sub,[<$name dist_sub>],[<$name local_sub>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchSub,[<$name dist_fetch_sub>],[<$name local_sub>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Mul,[<$name dist_mul>],[<$name local_mul>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchMul,[<$name dist_fetch_mul>],[<$name local_mul>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Div,[<$name dist_div>],[<$name local_div>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchDiv,[<$name dist_fetch_div>],[<$name local_div>]}

        }
    }
}

#[macro_export]
macro_rules! CollectiveAtomicArray_create_bitwise_ops {
    ($a:ty, $name:ident) => {
        paste::paste!{
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::And,[<$name dist_bit_and>],[<$name local_bit_and>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchAnd,[<$name dist_fetch_bit_and>],[<$name local_bit_and>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Or,[<$name dist_bit_or>],[<$name local_bit_or>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::FetchOr,[<$name dist_fetch_bit_or>],[<$name local_bit_or>]}
        }
    }
}

#[macro_export]
macro_rules! CollectiveAtomicArray_create_atomic_ops {
    ($a:ty, $name:ident) => {
        paste::paste!{
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Store,[<$name dist_store>],[<$name local_store>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Load,[<$name dist_load>],[<$name local_load>]}
            $crate::collectiveatomicarray_register!{$a,ArrayOpCmd::Swap,[<$name dist_swap>],[<$name local_swap>]}
        }
    }
}
#[macro_export]
macro_rules! collectiveatomicarray_register {
    ($id:ident, $optype:path, $op:ident, $local:ident) => {
        inventory::submit! {
            #![crate =$crate]
            $crate::array::CollectiveAtomicArrayOp{
                id: ($optype,std::any::TypeId::of::<$id>()),
                op: $op,
            }
        }
    };
}
