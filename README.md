Lamellar - Rust HPC runtime
=================================================

Lamellar is an investigation of the applicability of the Rust systems programming language for HPC as an alternative to C and C++, with a focus on PGAS approaches.

# Some Nomenclature
Through out this readme and API documentation (https://docs.rs/lamellar/latest/lamellar/) there are a few terms we end up reusing a lot, those terms and brief descriptions are provided below:
- `PE` - a processing element, typically a multi threaded process, for those familiar with MPI, it corresponds to a Rank.
    - Commonly you will create 1 PE per psychical CPU socket on your system, but it is just as valid to have multiple PE's per CPU
    - There may be some instances where `Node` (meaning a compute node) is used instead of `PE` in these cases they are interchangeable
- `World` - an abstraction representing your distributed computing system
    - consists of N PEs all capable of communicating with one another
- `Team` - A subset of the PEs that exist in the world
- `AM` - short for [Active Message](https://docs.rs/lamellar/latest/lamellar/active_messaging)
- `Collective Operation` - Generally means that all PEs (associated with a given distributed object) must explicitly participate in the operation, otherwise deadlock will occur.
    - e.g. barriers, construction of new distributed objects
- `One-sided Operation` - Generally means that only the calling PE is required for the operation to successfully complete.
    - e.g. accessing local data, waiting for local work to complete

# Features

Lamellar provides several different communication patterns and programming models to distributed applications, briefly highlighted below
## Active Messages
Lamellar allows for sending and executing user defined active messages on remote PEs in a distributed environment.
User first implement runtime exported trait (LamellarAM) for their data structures and then call a procedural macro [#\[lamellar::am\]](https://docs.rs/lamellar/latest/lamellar/active_messaging/attr.am.html) on the implementation.
The procedural macro produces all the necessary code to enable remote execution of the active message.
More details can be found in the [Active Messaging](https://docs.rs/lamellar/latest/lamellar/active_messaging) module documentation.

## Darcs (Distributed Arcs)
Lamellar provides a distributed extension of an [`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html) called a [Darc](https://docs.rs/lamellar/latest/lamellar/darc).
Darcs provide safe shared access to inner objects in a distributed environment, ensuring lifetimes and read/write accesses are enforced properly.
More details can be found in the [Darc](https://docs.rs/lamellar/latest/lamellar/darc) module documentation.

## PGAS abstractions

Lamellar also provides PGAS capabilities through multiple interfaces.

### LamellarArrays (Distributed Arrays)

The first is a high-level abstraction of distributed arrays, allowing for distributed iteration and data parallel processing of elements.
More details can be found in the [LamellarArray](https://docs.rs/lamellar/latest/lamellar/array) module documentation.

### Low-level Memory Regions

The second is a low level (unsafe) interface for constructing memory regions which are readable and writable from remote PEs.
Note that unless you are very comfortable/confident in low level distributed memory (and even then) it is highly recommended you use the LamellarArrays interface
More details can be found in the [Memory Region](https://docs.rs/lamellar/latest/lamellar/memregion) module documentation.

# Network Backends

Lamellar relies on network providers called Lamellae to perform the transfer of data throughout the system.
Currently three such Lamellae exist: 
- `local` -  used for single-PE (single system, single process) development (this is the default), 
- `shmem` -  used for multi-PE (single system, multi-process) development, useful for emulating distributed environments (communicates through shared memory)
- `rofi` - used for multi-PE (multi system, multi-process) distributed development, based on the Rust OpenFabrics Interface Transport Layer (ROFI) (<https://github.com/pnnl/rofi>).
    - By default support for Rofi is disabled as using it relies on both the Rofi C-library and the libfabrics library, which may not be installed on your system.
    - It can be enabled by adding ```features = ["enable-rofi"]``` to the lamellar entry in your `Cargo.toml` file

The long term goal for lamellar is that you can develop using the `local` backend and then when you are ready to run distributed switch to the `rofi` backend with no changes to your code.
Currently the inverse is true, if it compiles and runs using `rofi` it will compile and run when using `local` and `shmem` with no changes.

Additional information on using each of the lamellae backends can be found below in the `Running Lamellar Applications` section

Examples 
--------
Our repository also provides numerous examples highlighting various features of the runtime: <https://github.com/pnnl/lamellar-runtime/tree/master/examples>

Additionally, we are compiling a set of benchmarks (some with multiple implementations) that may be helpful to look at as well: <https://github.com/pnnl/lamellar-benchmarks/>

Below are a few small examples highlighting some of the features of lamellar, more in-depth examples can be found in the documentation for the various features.
# Selecting a Lamellae and constructing a lamellar world instance
You can select which backend to use at runtime as shown below:
```
use lamellar::Backend;
fn main(){
 let mut world = lamellar::LamellarWorldBuilder::new()
        .with_lamellae( Default::default() ) //if "enable-rofi" feature is active default is rofi, otherwise  default is `Local`
        //.with_lamellae( Backend::Rofi ) //explicity set the lamellae backend to rofi,
        //.with_lamellae( Backend::Local ) //explicity set the lamellae backend to local
        //.with_lamellae( Backend::Shmem ) //explicity set the lamellae backend to use shared memory
        .build();
}
```
or by setting the following envrionment variable:
```LAMELLAE_BACKEND="lamellae"``` where lamellae is one of `local`, `shmem`, or `rofi`.

# Creating and executing a Registered Active Message
Please refer to the [Active Messaging](https://docs.rs/lamellar/latest/lamellar/active_messaging) documentation for more details and examples
```
use lamellar::active_messaging::prelude::*;

#[AmData(Debug, Clone)] // `AmData` is a macro used in place of `derive` 
struct HelloWorld { //the "input data" we are sending with our active message
    my_pe: usize, // "pe" is processing element == a node
}

#[lamellar::am] // at a highlevel registers this LamellarAM implemenatation with the runtime for remote execution
impl LamellarAM for HelloWorld {
    async fn exec(&self) {
        println!(
            "Hello pe {:?} of {:?}, I'm pe {:?}",
            lamellar::current_pe, 
            lamellar::num_pes,
            self.my_pe
        );
    }
}

fn main(){
    let mut world = lamellar::LamellarWorldBuilder::new().build();
    let my_pe = world.my_pe();
    let num_pes = world.num_pes();
    let am = HelloWorld { my_pe: my_pe };
    for pe in 0..num_pes{
        world.exec_am_pe(pe,am.clone()); // explicitly launch on each PE
    }
    world.wait_all(); // wait for all active messages to finish
    world.barrier();  // synchronize with other PEs
    let request = world.exec_am_all(am.clone()); //also possible to execute on every PE with a single call
    world.block_on(request); //both exec_am_all and exec_am_pe return futures that can be used to wait for completion and access any returned result
}
```

# Creating, initializing, and iterating through a distributed array
Please refer to the [LamellarArray](https://docs.rs/lamellar/latest/lamellar/array) documentation for more details and examples
```
use lamellar::array::prelude::*;

fn main(){
    let world = lamellar::LamellarWorldBuilder::new().build();
    let my_pe = world.my_pe();
    let block_array = AtomicArray::<usize>::new(&world, 1000, Distribution::Block); //we also support Cyclic distribution.
    block_array.dist_iter_mut().enumerate().for_each(move |elem| *elem = my_pe); //simultaneosuly initialize array accross all PEs, each pe only updates its local data
    block_array.wait_all();
    block_array.barrier();
    if my_pe == 0{
        for (i,elem) in block_array.onesided_iter().into_iter().enumerate(){ //iterate through entire array on pe 0 (automatically transfering remote data)
            println!("i: {} = {})",i,elem);
        }
    }
}
```

# Utilizing a Darc within an active message
Please refer to the [Darc](https://docs.rs/lamellar/latest/lamellar/darc) documentation for more details and examples
```
use lamellar::active_messaging::prelude::*;
use std::sync::atomic::{AtomicUsize,Ordering};

#[AmData(Debug, Clone)] // `AmData` is a macro used in place of `derive` 
struct DarcAm { //the "input data" we are sending with our active message
    cnt: Darc<AtomicUsize>, // count how many times each PE executes an active message
}

#[lamellar::am] // at a highlevel registers this LamellarAM implemenatation with the runtime for remote execution
impl LamellarAM for DarcAm {
    async fn exec(&self) {
        self.cnt.fetch_add(1,Ordering::SeqCst);
    }
}

fn main(){
    let mut world = lamellar::LamellarWorldBuilder::new().build();
    let my_pe = world.my_pe();
    let num_pes = world.num_pes();
    let cnt = Darc::new(&world, AtomicUsize::new());
    for pe in 0..num_pes{
        world.exec_am_pe(pe,DarcAm{cnt: cnt.clone()}); // explicitly launch on each PE
    }
    world.exec_am_all(am.clone()); //also possible to execute on every PE with a single call
    cnt.fetch_add(1,Ordering::SeqCst); //this is valid as well!
    world.wait_all(); // wait for all active messages to finish
    world.barrier();  // synchronize with other PEs
    assert_eq!(cnt.load(Ordering::SeqCst),num_pes*2 + 1);
}
```
# Using Lamellar 
Lamellar is capable of running on single node workstations as well as distributed HPC systems.
For a workstation, simply copy the following to the dependency section of you Cargo.toml file:

``` lamellar = "0.5" ```

If planning to use within a distributed HPC system a few more steps may be necessary (this also works on single workstations):

1. ensure Libfabric (with support for the verbs provider) is installed on your system <https://github.com/ofiwg/libfabric> 
2. set the OFI_DIR environment variable to the install location of Libfabric, this directory should contain both the following directories:
    * lib
    * include
3. copy the following to your Cargo.toml file:

```lamellar = { version = "0.5", features = ["enable-rofi"]}```


For both environments, build your application as normal

```cargo build (--release)```
# Running Lamellar Applications
There are a number of ways to run Lamellar applications, mostly dictated by the lamellae you want to use.
## local (single-process, single system)
1. directly launch the executable
    - ```cargo run --release```
## shmem (multi-process, single system)
1. grab the [lamellar_run.sh](https://github.com/pnnl/lamellar-runtime/blob/master/lamellar_run.sh)
2. Use `lamellar_run.sh` to launch your application
    - ```./lamellar_run -N=2 -T=10 <appname>```
        - `N` number of PEs (processes) to launch (Default=1)
        - `T` number of threads Per PE (Default = number of cores/ number of PEs)
        - assumes `<appname>` executable is located at `./target/release/<appname>`
## rofi (multi-process, multi-system)
1. allocate compute nodes on the cluster:
    - ```salloc -N 2```
2. launch application using cluster launcher
    - ```srun -N 2 -mpi=pmi2 ./target/release/<appname>``` 
        - `pmi2` library is required to grab info about the allocated nodes and helps set up initial handshakes

# Environment Variables
Lamellar exposes a number of environment variables that can used to control application execution at runtime
- `LAMELLAR_THREADS` - The number of worker threads used within a lamellar PE
    -  `export LAMELLAR_THREADS=10`
- `LAMELLAE_BACKEND` - the backend used during execution. Note that if a backend is explicitly set in the world builder, this variable is ignored.
    - possible values
        - `local` 
        - `shmem` 
        - `rofi`
- `LAMELLAR_MEM_SIZE` - Specify the initial size of the Runtime "RDMAable" memory pool. Defaults to 1GB
    - `export LAMELLAR_MEM_SIZE=$((20*1024*1024*1024))` 20GB memory pool
    - Internally, Lamellar utilizes memory pools of RDMAable memory for Runtime data structures (e.g. [Darcs][crate::Darc], [OneSidedMemoryRegion][crate::memregion::OneSidedMemoryRegion],etc), aggregation buffers, and message queues. Additional memory pools are dynamically allocated across the system as needed. This can be a fairly expensive operation (as the operation is synchronous across all PEs) so the runtime will print a message at the end of execution with how many additional pools were allocated. 
        - if you find you are dynamically allocating new memory pools, try setting `LAMELLAR_MEM_SIZE` to a larger value
    - Note: when running multiple PEs on a single system, the total allocated memory for the pools would be equal to `LAMELLAR_MEM_SIZE * number of processes`

NEWS
----
* January 2023: Alpha release -- v0.5
* March 2022: Alpha release -- v0.4
* April 2021: Alpha release -- v0.3
* September 2020: Add support for "local" lamellae, prep for crates.io release -- v0.2.1
* July 2020: Second alpha release -- v0.2
* Feb 2020: First alpha release -- v0.1

BUILD REQUIREMENTS
------------------


* Crates listed in Cargo.toml


Optional:
Lamellar requires the following dependencies if wanting to run in a distributed HPC environment:
the rofi lamellae is enabled by adding "enable-rofi" to features either in cargo.toml or the command line when building. i.e. cargo build --features enable-rofi
Rofi can either be built from source and then setting the ROFI_DIR environment variable to the Rofi install directory, or by letting the rofi-sys crate build it automatically.

* [libfabric](https://github.com/ofiwg/libfabric) 
* [ROFI](https://github.com/pnnl/rofi)
* [rofi-sys](https://github.com/pnnl/rofi-sys) -- available in [crates.io](https://crates.io/crates/rofisys)


At the time of release, Lamellar has been tested with the following external packages:

| **GCC** | **CLANG** | **ROFI**  | **OFI**       | **IB VERBS**  | **MPI**       | **SLURM** |
|--------:|----------:|----------:|--------------:|--------------:|--------------:|----------:|
| 7.1.0   | 8.0.1     | 0.1.0     | 1.9.0 -1.14.0 | 1.13          | mvapich2/2.3a | 17.02.7   |

The OFI_DIR environment variable must be specified with the location of the OFI (libfabrics) installation.
The ROFI_DIR environment variable must be specified with the location of the ROFI installation (otherwise rofi-sys crate will build for you automatically).
(See https://github.com/pnnl/rofi for instructions installing ROFI (and libfabrics))

BUILDING PACKAGE
----------------
In the following, assume a root directory ${ROOT}

0. download Lamellar to ${ROOT}/lamellar-runtime

 `cd ${ROOT} && git clone https://github.com/pnnl/lamellar-runtime`

1. Select Lamellae to use:
    * In Cargo.toml add "enable-rofi" feature if wanting to use rofi (or pass --features enable-rofi to your cargo build command ), otherwise only support for local and shmem backends will be built.

2. Compile Lamellar lib and test executable (feature flags can be passed to command line instead of specifying in cargo.toml)

`cargo build (--release) (--features enable-rofi)`

    executables located at ./target/debug(release)/test

3. Compile Examples

`cargo build --examples (--release) (--features enable-rofi) `

    executables located at ./target/debug(release)/examples/

Note: we do an explicit build instead of `cargo run --examples` as they are intended to run in a distriubted envrionment (see TEST section below.)

HISTORY
-------
- version 0.5
  - Vastly improved documentation (i.e. it exists now ;))
  - 'Asyncified' the API - most remote operations now return Futures
  - LamellarArrays
    - Additional OneSidedIterators, LocalIterators, DistributedIterators
    - Additional element-wise operations
    - For Each "schedulers"
    - Backend optimizations
  - AM task groups
  - AM backend updates
  - Hooks for tracing
- version 0.4
  - Distributed Arcs (Darcs: distributed atomically reference counted objects)
  - LamellarArrays
    - UnsafeArray, AtomicArray, LocalLockArray, ReadOnlyArray, LocalOnlyArray
    - Distributed Iteration
    - Local Iteration
  - SHMEM backend
  - dynamic internal RDMA memory pools 
- version 0.3.0
  - recursive active messages
  - subteam support
  - support for custom team architectures (Examples/team_examples/custom_team_arch.rs)
  - initial support of LamellarArray (Am based collectives on distributed arrays)
  - integration with Rofi 0.2
  - revamped examples
- version 0.2.2:
  - Provide examples in readme
- version 0.2.1:
  - Provide the local lamellae as the default lamellae
  - feature guard rofi lamellae so that lamellar can build on systems without libfabrics and ROFI
  - added an example proxy app for doing a distributed DFT
- version 0.2:
  - New user facing API
  - Registered Active Messages (enabling stable rust)
  - Remote Closures feature guarded for use with nightly rust
  - redesigned internal lamellae organization
  - initial support for world and teams (sub groups of PE)
- version 0.1:
  - Basic init/finit functionalities
  - Remote Closure Execution
  - Basic memory management (heap and data section)
  - Basic Remote Memory Region Support (put/get)
  - ROFI Lamellae (Remote Closure Execution, Remote Memory Regions)
  - Sockets Lamellae (Remote Closure Execution, limited support for Remote Memory Regions)
  - simple examples
  
NOTES
-----

STATUS
------
Lamellar is still under development, thus not all intended features are yet
implemented.

CONTACTS
--------

Current Team Members

Ryan Friese     - ryan.friese@pnnl.gov  
Roberto Gioiosa - roberto.gioiosa@pnnl.gov
Erdal Mutlu     - erdal.mutlu@pnnl.gov  
Joseph Cottam   - joseph.cottam@pnnl.gov
Greg Roek       - gregory.roek@pnnl.gov

Past Team Members

Mark Raugas     - mark.raugas@pnnl.gov  

## License

This project is licensed under the BSD License - see the [LICENSE.md](LICENSE.md) file for details.

## Acknowledgments

This work was supported by the High Performance Data Analytics (HPDA) Program at Pacific Northwest National Laboratory (PNNL),
a multi-program DOE laboratory operated by Battelle.
