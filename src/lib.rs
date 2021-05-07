#![cfg_attr(feature = "nightly", feature(unboxed_closures))]

#[cfg(feature = "nightly")]
#[macro_use]
extern crate serde_closure;
#[cfg(feature = "nightly")]
pub use serde_closure::FnOnce;

#[macro_use]
extern crate lazy_static;

// TODO: maybe make a barrier trait?

// #[macro_use]
// extern crate crossbeam;

mod active_messaging;
mod lamellae;
mod lamellae_new;
mod lamellar_alloc;
mod lamellar_arch;
#[cfg(feature = "experimental")]
mod lamellar_array;
#[cfg(feature = "experimental")]
mod lamellar_darc;

mod barrier;

mod lamellar_memregion;
mod lamellar_request;
mod lamellar_team;
mod lamellar_world;
mod schedulers;
mod scheduler;
mod utils;
pub use utils::*;

#[doc(hidden)]
use lamellar_prof::init_prof;
init_prof!();

#[doc(hidden)]
pub use crate::active_messaging::{registered_active_message::RegisteredAm, LamellarReturn};

#[doc(hidden)]
pub use crate::active_messaging::{LamellarActiveMessage,LamellarSerde};

#[cfg(feature = "nightly")]
pub use crate::active_messaging::remote_closures::RemoteClosures;
pub use crate::active_messaging::{ActiveMessaging, LamellarAM,LocalAM};

#[cfg(feature = "experimental")]
pub use crate::lamellar_array::{LamellarArray, ReduceKey};
pub use crate::lamellar_memregion::{
    LamellarLocalMemoryRegion, LamellarMemoryRegion, RegisteredMemoryRegion, RemoteMemoryRegion,
};

#[cfg(feature = "experimental")]
pub use crate::lamellar_darc::{Darc,LocalRwDarc};
#[cfg(feature = "experimental")]
#[doc(hidden)]
pub use crate::lamellar_darc::{darc_serialize,darc_from_ndarc,localrw_serialize,localrw_from_ndarc};


pub use crate::lamellae::Backend;
pub use crate::schedulers::SchedulerType;

pub use crate::lamellar_world::*;

pub use crate::lamellar_arch::{BlockedArch, IdError, LamellarArch, StridedArch};

#[doc(hidden)]
pub use crate::lamellar_team::LamellarTeamRT;

pub use crate::lamellar_team::LamellarTeam;



extern crate lamellar_impl;
pub use lamellar_impl::{am, local_am, generate_reductions_for_type, reduction, register_reduction,AmData,AmLocalData};

#[doc(hidden)]
pub use inventory;

#[doc(hidden)]
pub use bincode;

#[doc(hidden)]
pub fn serialize<T: ?Sized>(obj: &T) -> Result<Vec<u8>, anyhow::Error>
where
    T: serde::Serialize,
{
    // let mut buf = Vec::new();
    // obj.serialize(&mut rmp_serde::Serializer::new(&mut buf)).unwrap()
    Ok(bincode::serialize(obj)?)
    // Ok(postcard::to_stdvec(obj)?)
    // Ok(rmp_serde::to_vec(obj)?)
}


// #[doc(hidden)]
// async fn serialize_with<T: ?Sized>(obj: &T,rdma: std::sync::Arc<dyn LamellaeRDMA>) -> Result<Vec<u8>, anyhow::Error>
// where
//     T: serde::Serialize,
// {
//     let size = bincode::serialize_size(obj)?;
//     let mut mem = rdma.rt_alloc(size)?;
//     while mem.is_none(){
//         async_std::task::yield_now().await;
//     }
//     let mut mem_slice = std::slice::from_raw_parts_mut(mem.unwrap() as *mut u8, size);
//     Ok(bincode::serialize_into(mem_slice,obj)?)

//     // let mut buf = Vec::new();
//     // obj.serialize(&mut rmp_serde::Serializer::new(&mut buf)).unwrap()
//     // Ok(bincode::serialize(obj)?)
//     // Ok(postcard::to_stdvec(obj)?)
//     // Ok(rmp_serde::to_vec(obj)?)
// }

#[doc(hidden)]
pub fn deserialize<'a, T>(bytes: &'a [u8]) -> Result<T, anyhow::Error>
where
    T: serde::Deserialize<'a>,
{
    Ok(bincode::deserialize(bytes)?)
    // Ok(postcard::from_bytes(bytes)?)
    // Ok(rmp_serde::from_read_ref(bytes)?)
}
#[doc(hidden)]
pub use async_std;
