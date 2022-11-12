use lamellar::array::prelude::*;
use rand::seq::SliceRandom;
use rand::SeedableRng;

// use tracing_flame::FlameLayer;
// use tracing_subscriber::{fmt, prelude::*, registry::Registry};

// fn setup_global_subscriber() -> impl Drop {
//     let fmt_layer = fmt::Layer::default();

//     let (flame_layer, _guard) = FlameLayer::with_file("./tracing.folded").unwrap();
//     let flame_layer = flame_layer.with_threads_collapsed(true);

//     let subscriber = Registry::default().with(fmt_layer).with(flame_layer);

//     tracing::subscriber::set_global_default(subscriber).expect("Could not set global default");
//     _guard
// }

fn main() {
    // let _guard = setup_global_subscriber();
    let world = lamellar::LamellarWorldBuilder::new().build();
    let num_pes = world.num_pes();
    let my_pe = world.my_pe();

    let array = AtomicArray::<usize>::new(world.team(), num_pes * 2, Distribution::Block);
    array.dist_iter_mut().for_each(|x| x.store(0)); //initialize array -- use atomic store
    array.wait_all();
    array.barrier();

    for i in 0..array.len() {
        let mut fail_cnt = 0;
        let old = my_pe;
        let new = my_pe + 1;
        while world.block_on(array.compare_exchange(i, old, new)).is_err() {
            //compare_exchange reutrns Option<Vec<Result<T,T>>>
            // the outer option should never be none,
            // vec is cause we can apply to multiple inidices in one call (see below),
            // inner result is whether the compare and exchange was successful
            fail_cnt += 1;
        }
        println!("fail_cnt {fail_cnt}");
    }
    array.barrier();
    array.print();

    let array_2 = AtomicArray::<f32>::new(world.team(), num_pes * 100000, Distribution::Cyclic);
    array_2.dist_iter_mut().for_each(|x| x.store(0.0));
    array_2.wait_all();
    array_2.barrier();

    let mut rng = rand::rngs::StdRng::seed_from_u64(my_pe as u64);
    let mut indices: Vec<usize> = (0..array_2.len()).collect();
    indices.shuffle(&mut rng);
    let old = 0.0;
    let new = (my_pe + 1) as f32;
    let epsilon = 0.00001;
    let res = world.block_on(array_2.batch_compare_exchange_epsilon(indices, old, new, epsilon)); //should not fail
    array_2.barrier();

    let (num_failed, num_ok) = res.iter().fold((0, 0), |acc, x| {
        if x.is_err() {
            (acc.0 + 1, acc.1)
        } else {
            (acc.0, acc.1 + 1)
        }
    });

    let array2_clone = array_2.clone();
    world.block_on(async move {
        let res = array2_clone
            .compare_exchange_epsilon(0, 10.0, 11.0, 0.1)
            .await;
        match res {
            Ok(_) => {
                println!("success");
            }
            Err(_) => {
                println!("failed");
            }
        }
    });

    let l = array.dist_iter().enumerate().for_each_async(move |(i, e)| {
        let a2c = array_2.clone();
        async move {
            let res = a2c
                .compare_exchange_epsilon(i, e.load() as f32, 0.0, epsilon)
                .await;
            match res {
                Ok(_) => {
                    println!("success");
                }
                Err(_) => {
                    println!("failed");
                }
            }
        }
    });
    world.block_on(l);
    println!("num_failed {num_failed} num_ok {num_ok}");
    // array2.print();
}
