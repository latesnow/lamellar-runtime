use crate::active_messaging::*;
use crate::array::native_atomic::*;
use crate::array::*;
use crate::lamellar_request::LamellarRequest;
// use crate::memregion::Dist;
use std::any::TypeId;
use std::collections::HashMap;

type OpFn = fn(*const u8, NativeAtomicByteArray, usize) -> LamellarArcAm;

lazy_static! {
    static ref OPS: HashMap<(ArrayOpCmd, TypeId), OpFn> = {
        let mut map = HashMap::new();
        for op in crate::inventory::iter::<NativeAtomicArrayOp> {
            map.insert(op.id.clone(), op.op);
        }
        map
    };
}

pub struct NativeAtomicArrayOp {
    pub id: (ArrayOpCmd, TypeId),
    pub op: OpFn,
}

crate::inventory::collect!(NativeAtomicArrayOp);

type BufFn = fn(NativeAtomicByteArray) -> Arc<dyn BufferOp>;

lazy_static! {
    pub(crate) static ref BUFOPS: HashMap<TypeId, BufFn> = {
        let mut map = HashMap::new();
        for op in crate::inventory::iter::<NativeAtomicArrayOpBuf> {
            map.insert(op.id.clone(), op.op);
        }
        map
    };
}

pub struct NativeAtomicArrayOpBuf {
    pub id: TypeId,
    pub op: BufFn,
}

crate::inventory::collect!(NativeAtomicArrayOpBuf);

impl<T: AmDist + Dist  + 'static> NativeAtomicArray<T> {
    pub(crate) fn initiate_op<'a>(
        &self,
        val: T,
        index: impl OpInput<'a,usize>,
        op: ArrayOpCmd,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.array.initiate_op( val, index, op)
    }

    pub(crate) fn initiate_fetch_op<'a>(
        &self,
        val: T,
        index: impl OpInput<'a,usize>,
        op: ArrayOpCmd,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.array.initiate_fetch_op( val, index, op)
    }

    pub fn store<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op( val, index, ArrayOpCmd::Store)
    }

    pub fn load<'a>(&self, index: impl OpInput<'a,usize>,) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        let dummy_val = self.array.dummy_val(); //we dont actually do anything with this except satisfy apis;
        self.initiate_fetch_op(dummy_val, index, ArrayOpCmd::Load)
    }

    pub fn swap<'a>(&self, index: impl OpInput<'a,usize>, val: T) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::Swap)
    }
}

impl<T: ElementArithmeticOps  + 'static> ArithmeticOps<T> for NativeAtomicArray<T> {
    fn add<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::Add)
    }
    fn fetch_add<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::FetchAdd)
    }
    fn sub<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::Sub)
    }
    fn fetch_sub<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::FetchSub)
    }
    fn mul<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::Mul)
    }
    fn fetch_mul<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::FetchMul)
    }
    fn div<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::Div)
    }
    fn fetch_div<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::FetchDiv)
    }
}

impl<T: ElementBitWiseOps  + 'static> BitWiseOps<T> for NativeAtomicArray<T> {
    fn bit_and<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::And)
    }
    fn fetch_bit_and<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op(val, index, ArrayOpCmd::FetchAnd)
    }

    fn bit_or<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = ()> + Send + Sync> {
        self.initiate_op(val, index, ArrayOpCmd::Or)
    }
    fn fetch_bit_or<'a>(
        &self,
        index: impl OpInput<'a,usize>,
        val: T,
    ) -> Box<dyn LamellarRequest<Output = Vec<T>> + Send + Sync> {
        self.initiate_fetch_op( val, index, ArrayOpCmd::FetchOr)
    }
}

// // impl<T: Dist + std::ops::AddAssign> NativeAtomicArray<T> {
// impl<T: ElementArithmeticOps> LocalArithmeticOps<T> for NativeAtomicArray<T> {
//     fn local_fetch_add(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         // println!("local_add LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         // let _lock = self.lock.write();
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         let orig = slice[index]; //this locks the
//         slice[index] += val;
//         orig
//     }
//     fn local_fetch_sub(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         // println!("local_sub LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         let orig = slice[index];
//         slice[index] -= val;
//         orig
//     }
//     fn local_fetch_mul(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         // println!("local_sub LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         let orig = slice[index];
//         slice[index] *= val;
//         orig
//     }
//     fn local_fetch_div(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         // println!("local_sub LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         let orig = slice[index];
//         slice[index] /= val;
//         // println!("div i: {:?} {:?} {:?} {:?}",index,orig,val,self.local_as_mut_slice()[index]);
//         orig
//     }
// }
// impl<T: ElementBitWiseOps> LocalBitWiseOps<T> for NativeAtomicArray<T> {
//     fn local_fetch_bit_and(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         // println!("local_sub LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         let orig = slice[index];
//         slice[index] &= val;
//         // println!("and i: {:?} {:?} {:?} {:?}",index,orig,val,self.local_as_mut_slice()[index]);
//         orig
//     }
//     fn local_fetch_bit_or(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         // println!("local_sub LocalArithmeticOps<T> for NativeAtomicArray<T> ");
//         let orig = slice[index];
//         slice[index] |= val;
//         orig
//     }
// }
// impl<T: ElementOps> LocalNativeAtomicOps<T> for NativeAtomicArray<T> {
//     fn local_load(&self, index: impl OpInput<'a,usize>, _val: T) -> T {
//         self.local_as_mut_slice()[index]
//     }

//     fn local_store(&self, index: impl OpInput<'a,usize>, val: T) {
//         self.local_as_mut_slice()[index] = val; //this locks the array
//     }

//     fn local_swap(&self, index: impl OpInput<'a,usize>, val: T) -> T {
//         let mut slice = self.local_as_mut_slice(); //this locks the array
//         let orig = slice[index];
//         slice[index] = val;
//         orig
//     }
// }
// // }

// #[macro_export]
// macro_rules! NativeAtomicArray_create_ops {
//     ($a:ty, $name:ident) => {
//         paste::paste!{
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Add,[<$name dist_add>],[<$name local_add>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchAdd,[<$name dist_fetch_add>],[<$name local_add>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Sub,[<$name dist_sub>],[<$name local_sub>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchSub,[<$name dist_fetch_sub>],[<$name local_sub>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Mul,[<$name dist_mul>],[<$name local_mul>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchMul,[<$name dist_fetch_mul>],[<$name local_mul>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Div,[<$name dist_div>],[<$name local_div>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchDiv,[<$name dist_fetch_div>],[<$name local_div>]}

//         }
//     }
// }

// #[macro_export]
// macro_rules! NativeAtomicArray_create_bitwise_ops {
//     ($a:ty, $name:ident) => {
//         paste::paste!{
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::And,[<$name dist_bit_and>],[<$name local_bit_and>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchAnd,[<$name dist_fetch_bit_and>],[<$name local_bit_and>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Or,[<$name dist_bit_or>],[<$name local_bit_or>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::FetchOr,[<$name dist_fetch_bit_or>],[<$name local_bit_or>]}
//         }
//     }
// }

// #[macro_export]
// macro_rules! NativeAtomicArray_create_atomic_ops {
//     ($a:ty, $name:ident) => {
//         paste::paste!{
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Store,[<$name dist_store>],[<$name local_store>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Load,[<$name dist_load>],[<$name local_load>]}
//             $crate::NativeAtomicArray_register!{$a,ArrayOpCmd::Swap,[<$name dist_swap>],[<$name local_swap>]}
//         }
//     }
// }
// #[macro_export]
// macro_rules! NativeAtomicArray_register {
//     ($id:ident, $optype:path, $op:ident, $local:ident) => {
//         inventory::submit! {
//             #![crate =$crate]
//             $crate::array::NativeAtomicArrayOp{
//                 id: ($optype,std::any::TypeId::of::<$id>()),
//                 op: $op,
//             }
//         }
//     };
// }
