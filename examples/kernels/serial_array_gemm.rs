/// ----------------Lamellar Serial Array GEMM---------------------------------------------------
/// This performs a disrtributed GEMM using the standard matrix multiplication algorithm with LamellarArrays
/// We only perform the multiplication on pe 0, serially (meaning a lot a data transfer occurs).
/// this is the simplest, but worst performing implementation we provide.
///----------------------------------------------------------------------------------
use lamellar::array::{DistributedIterator, Distribution, UnsafeArray};
use lamellar::ActiveMessaging;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let elem_per_pe = args
        .get(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| 20);

    let world = lamellar::LamellarWorldBuilder::new().build();
    let my_pe = world.my_pe();
    let num_pes = world.num_pes();

    let dim = elem_per_pe * num_pes;

    //for example purposes we are multiplying square matrices
    let m = dim; //a & c rows
    let n = dim; // a cols b rows
    let p = dim; // b & c cols

    let a = UnsafeArray::<f32>::new(&world, m * n, Distribution::Block); //row major
    let b = UnsafeArray::<f32>::new(&world, n * p, Distribution::Block); //col major
    let c = UnsafeArray::<f32>::new(&world, m * p, Distribution::Block); //row major
                                                                         //initialize matrices
    a.dist_iter_mut()
        .enumerate()
        .for_each(|(i, x)| *x = i as f32);
    b.dist_iter_mut().enumerate().for_each(move |(i, x)| {
        //identity matrix
        let row = i / dim;
        let col = i % dim;
        if row == col {
            *x = 1 as f32
        } else {
            *x = 0 as f32;
        }
    });
    c.dist_iter_mut().for_each(|x| *x = 0.0);

    world.wait_all();
    world.barrier();

    let num_gops = ((2 * dim * dim * dim) - dim * dim) as f64 / 1_000_000_000.0; // accurate for square matrices

    if my_pe == 0 {
        println!("starting");
    }

    //The standard unoptimized serial matrix muliply algorithm,
    let start = std::time::Instant::now();
    if my_pe == 0 {
        for i in 0..m {
            // a & c rows
            for j in 0..p {
                // b & c cols
                let mut sum = 0.0;
                for k in 0..n {
                    // a cols b rows
                    sum += a.at(k + i * m) * b.at(j + k * n)
                }
                c.put(j + i * m, sum); // could also do c.add(j+i*m,sum), but each element of c will only be updated once so put is faster
            }
        }
    }
    world.wait_all();
    world.barrier();
    let elapsed = start.elapsed().as_secs_f64();

    c.dist_iter_mut().for_each(|x| *x = 0.0);
    c.wait_all();
    c.barrier();
    println!("Elapsed: {:?}", elapsed);
    if my_pe == 0 {
        println!("elapsed {:?} Gflops: {:?}", elapsed, num_gops / elapsed,);
    }
}
