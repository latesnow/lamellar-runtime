use lamellar::array::prelude::*;
macro_rules! initialize_array {
    (UnsafeArray,$array:ident,$init_val:ident) => {
        unsafe {
            let _ = $array.dist_iter_mut().for_each(move |x| *x = $init_val);
        }
        $array.wait_all();
        $array.barrier();
    };
    (AtomicArray,$array:ident,$init_val:ident) => {
        let _ = $array.dist_iter().for_each(move |x| x.store($init_val));
        $array.wait_all();
        $array.barrier();
    };
    (LocalLockArray,$array:ident,$init_val:ident) => {
        let _ = $array.dist_iter_mut().for_each(move |x| *x = $init_val);
        $array.wait_all();
        $array.barrier();
    };
    (GlobalLockArray,$array:ident,$init_val:ident) => {
        let _ = $array.dist_iter_mut().for_each(move |x| *x = $init_val);
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
    (LocalLockArray,$val:ident,$max_val:ident,$valid:ident) => {
        if (($val - $max_val) as f32).abs() > 0.0001 {
            //all updates should be preserved
            $valid = false;
        }
    };
    (GlobalLockArray,$val:ident,$max_val:ident,$valid:ident) => {
        if (($val - $max_val) as f32).abs() > 0.0001 {
            //all updates should be preserved
            $valid = false;
        }
    };
}

macro_rules! max_updates {
    ($t:ty,$num_pes:ident) => {
        //calculate the log2 of the element type
        (std::mem::size_of::<u128>() * 8
            - (<$t>::MAX as u128 / $num_pes as u128).leading_zeros() as usize
            - 1 as usize)
            / $num_pes
    };
}

macro_rules! div_test{
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
            let one = 1 as $t;
            let init_val = max_val as $t;
            initialize_array!($array, array, init_val);
            array.wait_all();
            array.barrier();
            // array.print();
            for idx in 0..array.len(){
                for _i in 0..(max_updates as usize){
                    let _ = array.div(idx,2 as $t);
                }
            }
            array.wait_all();
            array.barrier();
            // array.print();
            #[allow(unused_unsafe)]
            for (i,elem) in unsafe {array.onesided_iter().into_iter().enumerate()}{
                let val = *elem;
                check_val!($array,val,one,success);
                if !success{
                    println!("full {:?} {:?} {:?}",i,val,one);
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
                for _i in 0..(max_updates as usize){
                    let _ = sub_array.div(idx,2 as $t);
                }
            }
            sub_array.wait_all();
            sub_array.barrier();
            #[allow(unused_unsafe)]
            for (i,elem) in unsafe {sub_array.onesided_iter().into_iter().enumerate()}{
                let val = *elem;
                check_val!($array,val,one,success);
                if !success{
                    println!("half {:?} {:?} {:?}",i,val,one);
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
                    for _i in 0..(max_updates as usize){
                        let _ = sub_array.div(idx,2 as $t);
                    }
                }
                sub_array.wait_all();
                sub_array.barrier();
                #[allow(unused_unsafe)]
                for (i,elem) in unsafe {sub_array.onesided_iter().into_iter().enumerate()}{
                    let val = *elem;
                    check_val!($array,val,one,success);
                    if !success{
                        println!("pe {:?} {:?} {:?}",i,val,one);
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
            "u8" => div_test!(UnsafeArray, u8, len, dist_type),
            "u16" => div_test!(UnsafeArray, u16, len, dist_type),
            "u32" => div_test!(UnsafeArray, u32, len, dist_type),
            "u64" => div_test!(UnsafeArray, u64, len, dist_type),
            "u128" => div_test!(UnsafeArray, u128, len, dist_type),
            "usize" => div_test!(UnsafeArray, usize, len, dist_type),
            "i8" => div_test!(UnsafeArray, i8, len, dist_type),
            "i16" => div_test!(UnsafeArray, i16, len, dist_type),
            "i32" => div_test!(UnsafeArray, i32, len, dist_type),
            "i64" => div_test!(UnsafeArray, i64, len, dist_type),
            "i128" => div_test!(UnsafeArray, i128, len, dist_type),
            "isize" => div_test!(UnsafeArray, isize, len, dist_type),
            "f32" => div_test!(UnsafeArray, f32, len, dist_type),
            "f64" => div_test!(UnsafeArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        "AtomicArray" => match elem.as_str() {
            "u8" => div_test!(AtomicArray, u8, len, dist_type),
            "u16" => div_test!(AtomicArray, u16, len, dist_type),
            "u32" => div_test!(AtomicArray, u32, len, dist_type),
            "u64" => div_test!(AtomicArray, u64, len, dist_type),
            "u128" => div_test!(AtomicArray, u128, len, dist_type),
            "usize" => div_test!(AtomicArray, usize, len, dist_type),
            "i8" => div_test!(AtomicArray, i8, len, dist_type),
            "i16" => div_test!(AtomicArray, i16, len, dist_type),
            "i32" => div_test!(AtomicArray, i32, len, dist_type),
            "i64" => div_test!(AtomicArray, i64, len, dist_type),
            "i128" => div_test!(AtomicArray, i128, len, dist_type),
            "isize" => div_test!(AtomicArray, isize, len, dist_type),
            "f32" => div_test!(AtomicArray, f32, len, dist_type),
            "f64" => div_test!(AtomicArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        "LocalLockArray" => match elem.as_str() {
            "u8" => div_test!(LocalLockArray, u8, len, dist_type),
            "u16" => div_test!(LocalLockArray, u16, len, dist_type),
            "u32" => div_test!(LocalLockArray, u32, len, dist_type),
            "u64" => div_test!(LocalLockArray, u64, len, dist_type),
            "u128" => div_test!(LocalLockArray, u128, len, dist_type),
            "usize" => div_test!(LocalLockArray, usize, len, dist_type),
            "i8" => div_test!(LocalLockArray, i8, len, dist_type),
            "i16" => div_test!(LocalLockArray, i16, len, dist_type),
            "i32" => div_test!(LocalLockArray, i32, len, dist_type),
            "i64" => div_test!(LocalLockArray, i64, len, dist_type),
            "i128" => div_test!(LocalLockArray, i128, len, dist_type),
            "isize" => div_test!(LocalLockArray, isize, len, dist_type),
            "f32" => div_test!(LocalLockArray, f32, len, dist_type),
            "f64" => div_test!(LocalLockArray, f64, len, dist_type),
            _ => eprintln!("unsupported element type"),
        },
        "GlobalLockArray" => match elem.as_str() {
            "u8" => div_test!(GlobalLockArray, u8, len, dist_type),
            "u16" => div_test!(GlobalLockArray, u16, len, dist_type),
            "u32" => div_test!(GlobalLockArray, u32, len, dist_type),
            "u64" => div_test!(GlobalLockArray, u64, len, dist_type),
            "u128" => div_test!(GlobalLockArray, u128, len, dist_type),
            "usize" => div_test!(GlobalLockArray, usize, len, dist_type),
            "i8" => div_test!(GlobalLockArray, i8, len, dist_type),
            "i16" => div_test!(GlobalLockArray, i16, len, dist_type),
            "i32" => div_test!(GlobalLockArray, i32, len, dist_type),
            "i64" => div_test!(GlobalLockArray, i64, len, dist_type),
            "i128" => div_test!(GlobalLockArray, i128, len, dist_type),
            "isize" => div_test!(GlobalLockArray, isize, len, dist_type),
            "f32" => div_test!(GlobalLockArray, f32, len, dist_type),
            "f64" => div_test!(GlobalLockArray, f64, len, dist_type),
            _ => {} //eprintln!("unsupported element type"),
        },
        _ => eprintln!("unsupported array type"),
    }
}
