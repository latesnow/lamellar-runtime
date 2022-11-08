use lamellar::array::{
    ArithmeticOps, AtomicArray, DistributedIterator, LocalLockAtomicArray, SerialIterator,
    UnsafeArray,
};

macro_rules! initialize_array {
    (UnsafeArray,$array:ident,$init_val:ident) => {
        $array.dist_iter_mut().for_each(move |x| *x = $init_val);
        $array.wait_all();
        $array.barrier();
    };
    (AtomicArray,$array:ident,$init_val:ident) => {
        $array.dist_iter().for_each(move |x| x.store($init_val));
        $array.wait_all();
        $array.barrier();
    };
    (LocalLockAtomicArray,$array:ident,$init_val:ident) => {
        $array.dist_iter_mut().for_each(move |x| *x = $init_val);
        $array.wait_all();
        $array.barrier();
    };
}

macro_rules! check_val {
    (UnsafeArray,$val:ident,$max_val:ident,$valid:ident) => {
        // UnsafeArray updates will be nondeterminstic so should not ever be considered safe/valid so for testing sake we just say they are
    };
    (AtomicArray,$val:ident,$max_val:ident,$valid:ident) => {
        if (($val - $max_val) as f32).abs() > 0.0001 {
            //all updates should be preserved
            $valid = false;
        }
    };
    (LocalLockAtomicArray,$val:ident,$max_val:ident,$valid:ident) => {
        if (($val - $max_val) as f32).abs() > 0.0001 {
            //all updates should be preserved
            $valid = false;
        }
    };
}

macro_rules! insert_prev{
    (UnsafeArray,$val:ident,$prevs:ident) => {
       // UnsafeArray updates will be nondeterminstic so should not ever be considered safe/valid so for testing sake we just say they are
       true
    };
    (AtomicArray,$val:ident,$prevs:ident) => {
        $prevs.insert($val)
    };
    (LocalLockAtomicArray,$val:ident,$prevs:ident) => {
        $prevs.insert($val)
    };
}

macro_rules! max_updates {
    ($t:ty,$num_pes:ident) => {
        // let temp: u128 = (<$t>::MAX as u128/$num_pes as u128);
        // (temp as f64).log(2.0) as usize

        (std::mem::size_of::<u128>() * 8
            - (<$t>::MAX as u128 / $num_pes as u128).leading_zeros() as usize
            - 1 as usize)
            / $num_pes
        // if <$t>::MAX as u128 > (10000 * $num_pes) as u128{
        //     10000
        // }
        // else{
        //     <$t>::MAX as usize / $num_pes
        // }
    };
}

macro_rules! fetch_mul_test{
    ($array:ident, $t:ty, $len:expr, $dist:ident) =>{
       {
            let world = lamellar::LamellarWorldBuilder::new().build();
            let num_pes = world.num_pes();
            let _my_pe = world.my_pe();
            let array_total_len = $len;
            #[allow(unused_mut)]
            let mut success = true;
            let array: $array::<$t> = $array::<$t>::new(world.team(), array_total_len, $dist).into(); //convert into abstract LamellarArray, distributed len is total_len

            let max_updates = max_updates!($t,num_pes);
            let max_val =  2u128.pow((max_updates*num_pes) as u32) as $t;
            let init_val = 1 as $t;
            initialize_array!($array, array, init_val);
            array.wait_all();
            array.barrier();
            // array.print();
            for idx in 0..array.len(){
                let mut reqs = vec![];
                for _i in 0..(max_updates as usize){
                    reqs.push(array.fetch_mul(idx,2 as $t));
                }
                #[allow(unused_mut)]
                let mut prevs: std::collections::HashSet<u128> = std::collections::HashSet::new();
                for req in reqs{
                    let val =  world.block_on(req) as u128;
                    if ! insert_prev!($array,val,prevs){
                        println!("full 1: {:?} {:?} {:?}",init_val,val,prevs);
                        success = false;
                        break;
                    }
                }
            }
            array.wait_all();
            array.barrier();
            // array.print();
            for (i,elem) in array.ser_iter().into_iter().enumerate(){
                let val = *elem;
                check_val!($array,val,max_val,success);
                if !success{
                    println!("{:?} {:?} {:?}",i,val,max_val);
                }
            }

            array.barrier();
            initialize_array!($array, array, init_val);


            let half_len = array_total_len/2;
            let start_i = half_len/2;
            let end_i = start_i + half_len;
            let sub_array = array.sub_array(start_i..end_i);
            sub_array.barrier();
            // // sub_array.print();
            for idx in 0..sub_array.len(){
                let mut reqs = vec![];
                for _i in 0..(max_updates as usize){
                    reqs.push(sub_array.fetch_mul(idx,2 as $t));
                }
                #[allow(unused_mut)]
                let mut prevs: std::collections::HashSet<u128>  = std::collections::HashSet::new();
                for req in reqs{
                    let val =  world.block_on(req) as u128;
                    if ! insert_prev!($array,val,prevs){
                        println!("half 1: {:?} {:?}",val,prevs);
                        success = false;
                        break;
                    }
                }
            }
            sub_array.wait_all();
            sub_array.barrier();
            for (i,elem) in sub_array.ser_iter().into_iter().enumerate(){
                let val = *elem;
                check_val!($array,val,max_val,success);
                if !success{
                    println!("{:?} {:?} {:?}",i,val,max_val);
                }
            }
            sub_array.barrier();
            initialize_array!($array, array, init_val);

            let pe_len = array_total_len/num_pes;
            for pe in 0..num_pes{
                let len = std::cmp::max(pe_len/2,1);
                let start_i = (pe*pe_len)+ len/2;
                let end_i = start_i+len;
                let sub_array = array.sub_array(start_i..end_i);
                sub_array.barrier();
                for idx in 0..sub_array.len(){
                    let mut reqs = vec![];
                    for _i in 0..(max_updates as usize){
                        reqs.push(sub_array.fetch_mul(idx,2 as $t));
                    }
                    #[allow(unused_mut)]
                    let mut prevs: std::collections::HashSet<u128>  = std::collections::HashSet::new();
                    for req in reqs{
                        let val =  world.block_on(req) as u128;
                        if ! insert_prev!($array,val,prevs){
                            println!("pe 1: {:?} {:?}",val,prevs);
                            success = false;
                            break;
                        }
                    }
                }
                sub_array.wait_all();
                sub_array.barrier();
                for (i,elem) in sub_array.ser_iter().into_iter().enumerate(){
                    let val = *elem;
                    check_val!($array,val,max_val,success);
                    if !success{
                        println!("{:?} {:?} {:?}",i,val,max_val);
                    }
                }
                sub_array.barrier();
                initialize_array!($array, array, init_val);
            }

            if !success{
                eprintln!("failed");
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let array = args[1].clone();
    let dist = args[2].clone();
    let elem = args[3].clone();
    let len = args[4].parse::<usize>().unwrap();

    let dist_type = match dist.as_str() {
        "Block" => lamellar::array::Distribution::Block,
        "Cyclic" => lamellar::array::Distribution::Cyclic,
        _ => panic!("unsupported dist type"),
    };

    match array.as_str() {
        "UnsafeArray" => match elem.as_str() {
            "u8" => fetch_mul_test!(UnsafeArray, u8, len, dist_type),
            "u16" => fetch_mul_test!(UnsafeArray, u16, len, dist_type),
            "u32" => fetch_mul_test!(UnsafeArray, u32, len, dist_type),
            "u64" => fetch_mul_test!(UnsafeArray, u64, len, dist_type),
            "u128" => fetch_mul_test!(UnsafeArray, u128, len, dist_type),
            "usize" => fetch_mul_test!(UnsafeArray, usize, len, dist_type),
            "i8" => fetch_mul_test!(UnsafeArray, i8, len, dist_type),
            "i16" => fetch_mul_test!(UnsafeArray, i16, len, dist_type),
            "i32" => fetch_mul_test!(UnsafeArray, i32, len, dist_type),
            "i64" => fetch_mul_test!(UnsafeArray, i64, len, dist_type),
            "i128" => fetch_mul_test!(UnsafeArray, i128, len, dist_type),
            "isize" => fetch_mul_test!(UnsafeArray, isize, len, dist_type),
            "f32" => fetch_mul_test!(UnsafeArray, f32, len, dist_type),
            "f64" => fetch_mul_test!(UnsafeArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        "AtomicArray" => match elem.as_str() {
            "u8" => fetch_mul_test!(AtomicArray, u8, len, dist_type),
            "u16" => fetch_mul_test!(AtomicArray, u16, len, dist_type),
            "u32" => fetch_mul_test!(AtomicArray, u32, len, dist_type),
            "u64" => fetch_mul_test!(AtomicArray, u64, len, dist_type),
            "u128" => fetch_mul_test!(AtomicArray, u128, len, dist_type),
            "usize" => fetch_mul_test!(AtomicArray, usize, len, dist_type),
            "i8" => fetch_mul_test!(AtomicArray, i8, len, dist_type),
            "i16" => fetch_mul_test!(AtomicArray, i16, len, dist_type),
            "i32" => fetch_mul_test!(AtomicArray, i32, len, dist_type),
            "i64" => fetch_mul_test!(AtomicArray, i64, len, dist_type),
            "i128" => fetch_mul_test!(AtomicArray, i128, len, dist_type),
            "isize" => fetch_mul_test!(AtomicArray, isize, len, dist_type),
            "f32" => fetch_mul_test!(AtomicArray, f32, len, dist_type),
            "f64" => fetch_mul_test!(AtomicArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        "LocalLockAtomicArray" => match elem.as_str() {
            "u8" => fetch_mul_test!(LocalLockAtomicArray, u8, len, dist_type),
            "u16" => fetch_mul_test!(LocalLockAtomicArray, u16, len, dist_type),
            "u32" => fetch_mul_test!(LocalLockAtomicArray, u32, len, dist_type),
            "u64" => fetch_mul_test!(LocalLockAtomicArray, u64, len, dist_type),
            "u128" => fetch_mul_test!(LocalLockAtomicArray, u128, len, dist_type),
            "usize" => fetch_mul_test!(LocalLockAtomicArray, usize, len, dist_type),
            "i8" => fetch_mul_test!(LocalLockAtomicArray, i8, len, dist_type),
            "i16" => fetch_mul_test!(LocalLockAtomicArray, i16, len, dist_type),
            "i32" => fetch_mul_test!(LocalLockAtomicArray, i32, len, dist_type),
            "i64" => fetch_mul_test!(LocalLockAtomicArray, i64, len, dist_type),
            "i128" => fetch_mul_test!(LocalLockAtomicArray, i128, len, dist_type),
            "isize" => fetch_mul_test!(LocalLockAtomicArray, isize, len, dist_type),
            "f32" => fetch_mul_test!(LocalLockAtomicArray, f32, len, dist_type),
            "f64" => fetch_mul_test!(LocalLockAtomicArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        _ => eprintln!("unsupported array type"),
    }
}
