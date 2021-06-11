use crate::active_messaging::*;
//{
    // ActiveMessageEngine, Cmd, ExecType, LamellarBoxedAm, LamellarReturn, Msg, RetType,
    // REQUESTS,LamellarBoxedData,
// };
use crate::lamellae::{Lamellae,LamellaeAM,SerializeHeader,SerializedData,Ser,Des,SubData};
use crate::lamellar_request::*;
use crate::lamellar_team::LamellarTeamRT;
use crate::scheduler::{NewReqData,AmeSchedulerQueue};
#[cfg(feature = "enable-prof")]
use lamellar_prof::*;
use log::trace;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::collections::HashSet;
use futures::future::join_all;
use async_recursion::async_recursion;

// enum BatchAmId{
const UNIT_ID: AmId = 0;
const BATCHED_UNIT_ID: AmId = UNIT_ID + 1;
const REMOTE_DATA_ID: AmId = BATCHED_UNIT_ID + 1;
const BATCHED_REMOTE_DATA_ID: AmId = REMOTE_DATA_ID + 1;
const REMOTE_AM_ID: AmId = BATCHED_REMOTE_DATA_ID + 1; //when returning an am as a result we pass the negative of its actual id
const AM_ID_START: AmId = REMOTE_AM_ID + 1;

pub (crate) type UnpackFn = fn(&[u8]) -> LamellarArcAm;
pub(crate) type AmId = i32;
lazy_static! {
    pub(crate) static ref AMS_IDS: HashMap<String, AmId> = {
        
        let mut ams = vec![];
        for am in crate::inventory::iter::<RegisteredAm>{
            ams.push(am.name.clone());
        }
        ams.sort();
        let mut cnt = AM_ID_START; 
        let mut temp = HashMap::new();
        for am in ams{
            temp.insert(am.clone(),cnt);
            cnt+=1;
        }
        temp
    };
}
lazy_static!{
    pub(crate) static ref AMS_EXECS: HashMap<AmId, UnpackFn> = {
        let mut temp = HashMap::new();
        for exec in crate::inventory::iter::<RegisteredAm> {
            // trace!("{:#?}", exec.name);
            let id = AMS_IDS.get(&exec.name).unwrap();
            temp.insert(*id, exec.exec);
        }
        temp
    };
}

// #[derive(Debug)]
pub struct RegisteredAm {
    pub exec: UnpackFn,
    pub name: String,
}
crate::inventory::collect!(RegisteredAm);


//TODO: we actually do need to group by team as well,
// so need to add the team hash hashmap back and move total size into there instead.
pub(crate) struct RegisteredActiveMessages{
    submitted_ams:  Arc<
                        Mutex<
                            HashMap<
                                Option<usize>, //pe
                                (
                                    HashMap<
                                        u64, //team hash
                                        (
                                            HashMap<
                                                AmId, //func id
                                                (
                                                    HashMap< 
                                                        Option<usize>, //batch_id
                                                        (
                                                            Vec<(usize,LamellarFunc)>, //req_id, am
                                                            usize, //batch size
                                                        )
                                                    >
                                                    ,usize//func size 
                                                )                     
                                            >,
                                            usize,//team size
                                        )
                                    >,
                                    usize,//total size                               
                                )
                            >
                        >
                    >, //pe, team hash, function id, batch id : (req_id,function), total data size // maybe we can remove the func id lookup if we enfore that a batch only contains the same function.. which I think we do...
    txed_ams: Arc<Mutex<HashMap<
                            usize, //batched req id
                            Mutex<HashMap<usize,usize>>>>>, //actual ids
    cur_batch_id: Arc<AtomicUsize>,
    scheduler: Arc<AmeScheduler>,
}

type TeamHeader = (usize,u64);
type FuncHeader = (usize,AmId);
type BatchHeader = (usize,usize);

impl RegisteredActiveMessages{
    pub(crate) fn new(scheduler: Arc<AmeScheduler>) -> RegisteredActiveMessages{
        RegisteredActiveMessages{
            submitted_ams:  Arc::new(Mutex::new(HashMap::new())),
            txed_ams: Arc::new(Mutex::new(HashMap::new())),
            cur_batch_id: Arc::new(AtomicUsize::new(1)),
            scheduler: scheduler,
        }
    }

    // right now we batch by (destination pe, team, func) triplets
    // need to analyze the benefits of different levels of batching (e.g. just batch based on pe)
    fn add_req_to_batch(&self,
        func: LamellarFunc, 
        func_size: usize, 
        func_id: AmId, 
        req_data: Arc<NewReqData>,){
        // println!("adding req {:?} {:?}",func_id,func_size);
        let team_header_len = crate::serialized_size::<TeamHeader>(&(0,0));
        let func_header_len = crate::serialized_size::<FuncHeader>(&(0,0));
        let batch_header_len = crate::serialized_size::<BatchHeader>(&(0,0));
        // add request to a batch or create a new one
        let mut submit_tx_task = false;
        let mut map = self.submitted_ams.lock();   
        let mut pe_entry = map.entry(req_data.dst).or_insert_with(|| { 
            // println!("going to submit tx task {:?}",func_id);
            submit_tx_task = true;
            (HashMap::new(),0)
        }); // pe
        pe_entry.1 += func_size;

        let mut team_entry = pe_entry.0.entry(req_data.team_hash).or_insert_with(|| {
            // println!("going to submit new team {:?}",req_data.team_hash);
            (HashMap::new(),0)
        }); // team
        if team_entry.1 == 0 {
            pe_entry.1+=team_header_len;
        }
        team_entry.1 += func_size;

        let mut func_entry= team_entry.0.entry(func_id).or_insert_with(|| {
            // println!("going to submit new func {:?}",func_id);
            (HashMap::new(),0)
        }); //func
        if func_entry.1 ==0 {
            pe_entry.1 += func_header_len;
            team_entry.1 += func_header_len;
        }
        func_entry.1 += func_size;

        let mut batch_entry = func_entry.0.entry(req_data.batch_id).or_insert_with( || {
            // println!("going to submit new batch {:?}",req_data.batch_id);
            (Vec::new(),0)
        }); // batch id        
        if batch_entry.1 == 0{
            pe_entry.1 += batch_header_len;
            team_entry.1 += batch_header_len;
            func_entry.1 += batch_header_len;
        }
        batch_entry.1 += func_size;
        batch_entry.0.push((req_data.id,func));
        drop(map); 
        //--------------------------

        if submit_tx_task{
            let submitted_ams = self.submitted_ams.clone();
            let txed_ams = self.txed_ams.clone();
            let outgoing_batch_id = self.cur_batch_id.clone();//fetch_add(1, Ordering::Relaxed);
            // println!{"submitting tx_task {:?} {:?}",outgoing_batch_id,req_data.cmd};
            self.scheduler.submit_task( async move{
                let mut cnt: usize=0;                       // -- this is a poor mans rate limiter
                while cnt < 10000{                          // essentially we want to make sure we
                    async_std::task::yield_now().await;     // buffer enough requests to make the
                    cnt+=1;                                 // batching worth it but also quick response time
                }                                           // ...can definitely do better
                let team_map = { //all requests going to pe
                    let mut map = submitted_ams.lock();
                    map.remove(&req_data.dst)
                };    
                // println!{"in submit_tx_task {:?} {:?}",outgoing_batch_id,req_data.cmd};
                if let Some((team_map,total_size)) = team_map{
                    // let header_size = std::mem::size_of::<Option<SerializeHeader>>();
                    // let agg_size = total_size + (func_map.len()-1) * header_size;
                    let msg = Msg {
                        cmd: ExecType::Am(Cmd::BatchedMsg), 
                        src: req_data.team.world_pe as u16, //this should always originate from me?
                        req_id: 0,//outgoing_batch_id.fetch_add(1, Ordering::Relaxed),
                    };
                    let header = Some(SerializeHeader{msg: msg, team_hash: 0, id: 0});
                    let data = req_data.lamellae.serialize_header(header,total_size).await.unwrap();
                    let data_slice = data.data_as_bytes();
                    let mut i = 0;
                    for (team_hash,(func_map,team_size)) in team_map{
                        let team_header: TeamHeader = (team_size,team_hash);
                        // println!("team_header: {:?}",team_header);
                        crate::serialize_into(&mut data_slice[i..i+team_header_len],&team_header).unwrap();
                        i+=team_header_len;
                        for (func_id,(batch_map,func_size)) in func_map{
                            let func_header: FuncHeader = (func_size,func_id);
                            // println!("func_header: {:?}",func_header);
                            crate::serialize_into(&mut data_slice[i..i+func_header_len],&func_header).unwrap();
                            i+=func_header_len;
                            for (batch_id,(reqs,batch_size)) in batch_map{
                                let batch_id = if let Some(batch_id) = batch_id { //batch exists
                                    batch_id
                                }
                                else{ 
                                    if func_id > 0 { //create new batch id
                                        outgoing_batch_id.fetch_add(1, Ordering::Relaxed)
                                    }else{ //original message not part of a batch
                                        0
                                    }
                                };
                                let batch_header: BatchHeader = (batch_size,batch_id);
                                // println!("batch_header: {:?}",batch_header);
                                crate::serialize_into(&mut data_slice[i..i+batch_header_len],&batch_header).unwrap();
                                i+=batch_header_len;
                              
                                let mut req_ids: HashMap<usize,usize> = HashMap::new();
                                let mut batch_req_id =0;
                                for (req_id,func) in reqs{
                                    // println!("req_id {:?} func_id {:?} batch_id {:?}",req_id,func_id,batch_id);
                                    match func_id {
                                        BATCHED_UNIT_ID  => {  }, //dont have to do anything special, as all we need to know is batch id
                                        UNIT_ID => {  
                                            let serialize_size = crate::serialized_size(&req_id);
                                            crate::serialize_into(&mut data_slice[i..i+serialize_size],&req_id).unwrap();
                                            i+=serialize_size;
                                        }
                                        REMOTE_DATA_ID => {  
                                            if let LamellarFunc::Result(func) = func{
                                                let result_header_size = crate::serialized_size(&(&req_id,0usize));
                                                    let serialize_size = func.serialized_size();
                                                    crate::serialize_into(&mut data_slice[i..i+serialize_size],&(req_id,serialize_size)).unwrap();
                                                    i+=result_header_size;                                            
                                                    func.serialize_into(&mut data_slice[i..(i+serialize_size)]);
                                                    // if req_data.dst.is_none() {
                                                    //     func.ser(req_data.team.num_pes()-1);
                                                    // }
                                                    // else{
                                                    //     func.ser(1);
                                                    // };
                                                    // ids.push(*req_id);
                                                    i+=serialize_size;
                                            }
                                        }
                                        BATCHED_REMOTE_DATA_ID => { 
                                            if let LamellarFunc::Result(func) = func{
                                                let result_header_size = crate::serialized_size(&(&req_id,0usize));
                                                    let serialize_size = func.serialized_size();
                                                    crate::serialize_into(&mut data_slice[i..(i+result_header_size)],&(req_id,serialize_size)).unwrap();
                                                    i+=result_header_size;                                            
                                                    func.serialize_into(&mut data_slice[i..(i+serialize_size)]);
                                                    // if req_data.dst.is_none() {
                                                    //     func.ser(req_data.team.num_pes()-1);
                                                    // }
                                                    // else{
                                                    //     func.ser(1);
                                                    // };
                                                    // ids.push(*req_id);
                                                    i+=serialize_size;
                                            }
    
                                        }
                                        _ => {
                                            match func{
                                                LamellarFunc::Am(func) => {
                                                    if func_id > 0 {
                                                        req_ids.insert(batch_req_id,req_id);
                                                        batch_req_id+=1;
                                                    }
                                                    else{
                                                        let serialized_size = crate::serialized_size(&0usize);
                                                        crate::serialize_into(&mut data_slice[i..(i+serialized_size)],&req_id).unwrap();
                                                        i+=serialized_size;
                                                    }
                                                    let serialize_size = func.serialized_size();
                                                    func.serialize_into(&mut data_slice[i..(i+serialize_size)]);
                                                    if req_data.dst.is_none() {
                                                        func.ser(req_data.team.num_pes()-1);
                                                    }
                                                    else{
                                                        func.ser(1);
                                                    };
                                                    
                                                    i+=serialize_size;
                                                },
                                                
                                                LamellarFunc::None =>{
                                                    panic!("should not be none"); //user registered function
                                                },
                                                _ =>{
                                                    panic!("not handled yet");
                                                }
    
                                            }
                                        } 
                                    }
                                }
                                if req_ids.len() > 0 { //only when we are sending initial requests, not return requests
                                    // println!("inserting batch_id {:?} {:?}",batch_id,req_ids);
                                    txed_ams.lock().insert(batch_id,Mutex::new(req_ids)); 
                                }
                            }
                        }
                    }                  
                    // println!("sending batch {:?}",data.header_and_data_as_bytes());
                    req_data.lamellae.send_to_pes_async(req_data.dst, req_data.team.arch.clone(), data).await;
                }
                // println!("leaving tx task");
            });
        }
        
    }

    
    async fn send_req(&self,
        func: LamellarFunc, 
        func_size: usize, 
        func_id: AmId, 
        req_data: Arc<NewReqData>,){
        
        let batch_id = if let Some(batch_id) = &req_data.batch_id{
            *batch_id
        }
        else{
            req_data.id
        };
        let msg = Msg {
            cmd: req_data.cmd, 
            src: req_data.team.world_pe as u16, //this should always originate from me?
            req_id: batch_id,
        };
        // println!("sending req {:?} {:?} {:?}",req_data.id,func_id,func_size);
        let header = Some(SerializeHeader{msg: msg, team_hash: req_data.team_hash, id: func_id});
        let data = req_data.lamellae.serialize_header(header,func_size).await.unwrap();
        let data_slice = data.data_as_bytes();
        match func{
            LamellarFunc::Am(func) => {
                if req_data.dst.is_none() {
                    func.ser(req_data.team.num_pes()-1);
                }
                else{
                    func.ser(1);
                };
                let mut i =0;
                if func_id < 0{
                    let serialized_size = crate::serialized_size(&0usize);
                    crate::serialize_into(&mut data_slice[i..i+serialized_size],&req_data.id).unwrap();
                    i += serialized_size;
                }
                func.serialize_into(&mut data_slice[i..])
            },
            LamellarFunc::Result(func) => {
                // if req_data.dst.is_none() {
                //     func.ser(req_data.team.num_pes()-1);
                // }
                // else{
                //     func.ser(1);
                // };
                let mut i =0;
                let serialized_size = crate::serialized_size(&(0usize,0usize));
                crate::serialize_into(&mut data_slice[i..i+serialized_size],&(req_data.id,func_size-serialized_size)).unwrap();
                i += serialized_size;
                func.serialize_into(&mut data_slice[i..])
            },
            LamellarFunc::None => panic!("should not send none")
        }
        req_data.lamellae.send_to_pes_async(req_data.dst, req_data.team.arch.clone(), data).await;
    }

    pub (crate) async fn process_am_req (
        &self,
        mut req_data: NewReqData,
       ){ 
        let my_pe = if let Ok(my_pe) = req_data.team.arch.team_pe(req_data.src) {
            Some(my_pe)
        } else {
            None
        };

        if req_data.dst == my_pe && my_pe != None {
            trace!("[{:?}] single local request ", my_pe);
           
            RegisteredActiveMessages::exec_local(
                Arc::new(req_data)
            ).await;
        }
        else{
            // println!("precessing req cmd {:?}",req_data.cmd);
            let (func_id, func_size, cmd) = match &req_data.func{
                LamellarFunc::Am(ref func) => {
                    let (func_id,func_size) = match &req_data.cmd{
                        ExecType::Am(Cmd::BatchedAmReturn) =>  (-(*AMS_IDS.get(&func.get_id()).unwrap()),func.serialized_size() +crate::serialized_size(&0usize)),
                        ExecType::Am(Cmd::AmReturn) =>  (-(*AMS_IDS.get(&func.get_id()).unwrap()),func.serialized_size() +crate::serialized_size(&0usize)),
                        _ => (*(AMS_IDS.get(&func.get_id()).unwrap()),func.serialized_size())
                    };
                    (func_id,func_size,ExecType::Am(Cmd::BatchedMsg))
                }
                LamellarFunc::Result(ref func) => {
                    let func_size = func.serialized_size()+crate::serialized_size(&(&req_data.id,0usize));
                    match req_data.cmd{
                        
                        ExecType::Am(Cmd::BatchedDataReturn)=>{
                            (BATCHED_REMOTE_DATA_ID,func_size,ExecType::Am(Cmd::BatchedDataReturn))
                        }
                        ExecType::Am(Cmd::DataReturn)=>{
                            (REMOTE_DATA_ID,func_size,ExecType::Am(Cmd::DataReturn))
                        }
                        _ => panic!("not handled yet")
                    }
                    
                },
                LamellarFunc::None => {
                    match req_data.cmd{
                        ExecType::Am(Cmd::UnitReturn) => (UNIT_ID, crate::serialized_size(&req_data.id),ExecType::Am(Cmd::UnitReturn)),
                        ExecType::Am(Cmd::BatchedUnitReturn) => (BATCHED_UNIT_ID, 0 ,ExecType::Am(Cmd::BatchedUnitReturn)),
                        _ => panic!("not handled yet")
                    }
                },
                _ => panic!("should only process AMS"),
            };               
            trace!("[{:?}] remote request ", my_pe);           
            
            let req_data = if func_size <= 10000{
                req_data.cmd=cmd;
                let req_data = Arc::new(req_data);
                self.add_req_to_batch(req_data.func.clone(),func_size,func_id,req_data.clone());//,Cmd::BatchedMsg);
                req_data
            }
            else{
                let req_data = Arc::new(req_data);
                self.send_req(req_data.func.clone(),func_size,func_id,req_data.clone()).await;
                req_data
            };

            if req_data.dst == None && my_pe != None {
                self.scheduler.submit_task(async move {
                    
                    RegisteredActiveMessages::exec_local(
                        req_data
                    ).await;
                });
            }
        }
        
    }
    
    #[async_recursion]
    async fn exec_local(
        req_data: Arc<NewReqData>,){
        if let  LamellarFunc::Am(func) = req_data.func.clone() {
            match func.exec(req_data.team.world_pe, req_data.team.num_world_pes, true, req_data.world.clone(), req_data.team.clone()).await {
                LamellarReturn::LocalData(data) => {
                    // println!("local am data return");
                    ActiveMessageEngine::send_data_to_user_handle(req_data.id,req_data.src as u16,InternalResult::Local(data));
                }
                LamellarReturn::LocalAm(am) => {
                    // println!("local am am return");
                    let req_data = Arc::new(req_data.copy_with_func(am));
                    RegisteredActiveMessages::exec_local(
                        req_data
                    ).await;
                    // exec_return_am(ame, msg, am, ireq, world, team).await;
                }
                LamellarReturn::Unit => {
                    // println!("local am unit return");
                    ActiveMessageEngine::send_data_to_user_handle(req_data.id,req_data.src as u16,InternalResult::Unit);
                }
                LamellarReturn::RemoteData(_) => {
                    // println!("remote am data return");
                    panic!("should not be returning remote data from local am");
                }
                LamellarReturn::RemoteAm(_) => {
                    // println!("remote am am return");
                    panic!("should not be returning remote am from local am");
                }   
            }
        }
        else{
            panic!("should only exec local ams");
        }
    }

    async fn exec_single_msg(&self,
        ame: Arc<ActiveMessageEngine>,
        msg: Msg, 
        ser_data: SerializedData, 
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>,
        return_am: bool,){
        if let Some(header) = ser_data.deserialize_header(){
            let func = AMS_EXECS.get(&(header.id)).unwrap()(ser_data.data_as_bytes());
            let lam_return = func.exec( team.world_pe, team.num_world_pes, return_am , world.clone(), team.clone()).await;
            match lam_return{
                LamellarReturn::Unit =>{  
                    let req_data = NewReqData{
                        src: team.world_pe ,
                        dst: Some(msg.src as usize),
                        cmd: ExecType::Am(Cmd::UnitReturn),
                        id: msg.req_id,
                        batch_id: None,
                        func:  LamellarFunc::None,
                        lamellae: lamellae,
                        world: world,
                        team: team,
                        team_hash: header.team_hash,
                    };
                    ame.process_msg_new(req_data, None).await;
                }
                LamellarReturn::LocalData(_) | LamellarReturn::LocalAm(_) =>{
                    panic!("Should not be returning local data from remote  am");
                }
                LamellarReturn::RemoteAm(func) => {
                    let req_data = NewReqData{
                        src: team.world_pe ,
                        dst: Some(msg.src as usize),
                        cmd: ExecType::Am(Cmd::AmReturn),
                        id: msg.req_id,
                        batch_id: None,
                        func:  LamellarFunc::Am(func),
                        lamellae: lamellae,
                        world: world,
                        team: team,
                        team_hash: header.team_hash,
                    };
                    ame.process_msg_new(req_data, None).await;
                }
                LamellarReturn::RemoteData(d) => {
                    let req_data = NewReqData{
                        src: team.world_pe ,
                        dst: Some(msg.src as usize),
                        cmd: ExecType::Am(Cmd::DataReturn),
                        id: msg.req_id,
                        batch_id: None,
                        func:  LamellarFunc::Result(d),
                        lamellae: lamellae,
                        world: world,
                        team: team,
                        team_hash: header.team_hash,
                    };
                    ame.process_msg_new(req_data, None).await;
                },
                _ => {panic!("unandled return type ")}
            }
        }
    }

    fn exec_batched_msg(&self,
        ame: Arc<ActiveMessageEngine>,
        msg: Msg, 
        ser_data: SerializedData, 
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>) {
        if let Some(header) = ser_data.deserialize_header(){
            // trace!("exec batched message {:?}",header.id);
            let data_slice=ser_data.data_as_bytes();
            let team_header_len = crate::serialized_size::<TeamHeader>(&(0,0));
            let func_header_len = crate::serialized_size::<FuncHeader>(&(0,0));
            let batch_header_len = crate::serialized_size::<BatchHeader>(&(0,0));
            let mut i = 0;
            while i < data_slice.len(){
                let team_header: TeamHeader = crate::deserialize(&data_slice[i..i+team_header_len]).unwrap();
                i += team_header_len;
                let team_start = i;
                let team_hash = team_header.1;
                while i < team_start + team_header.0{
                    let func_header: FuncHeader = crate::deserialize(&data_slice[i..i+func_header_len]).unwrap();
                    i += func_header_len;
                    let func_start = i;
                    let func_id = func_header.1;
                    while i < func_start + func_header.0{
                        let batch_header: BatchHeader = crate::deserialize(&data_slice[i..i+batch_header_len]).unwrap();
                        // println!("th {:?} fh {:?} bh {:?}",team_header,func_header,batch_header);
                        i += batch_header_len;
                        let batch_start = i;
                        let batch_id = batch_header.1;
                        let batched_data = &data_slice[i..i+batch_header.0];
                        // println!("batched_data {:?} {:?}",batched_data.len(),&batched_data);
                        let sub_data = ser_data.sub_data(i,i+batch_header.0);
                        i+=batch_header.0;
                        match func_id{
                            BATCHED_UNIT_ID  => self.process_batched_unit_return(batch_id, msg.src),
                            UNIT_ID => self.process_unit_return( msg.src, batched_data),
                            REMOTE_DATA_ID => self.process_data_return(msg, sub_data),
                            BATCHED_REMOTE_DATA_ID => self.process_batched_data_return(batch_id, msg.src, sub_data),
                            REMOTE_AM_ID => panic! {"not handled yet {:?}",func_id},
                            _ => {
                                if func_id > 0 {
                                    self.exec_batched_am(ame.clone(),msg.src as usize,team_hash,func_id,batch_id,lamellae.clone(),world.clone(),team.clone(),batched_data);
                                }
                                else{
                                    self.exec_batched_return_am(msg.src as usize,team_hash,-func_id,batch_id,lamellae.clone(),world.clone(),team.clone(),batched_data)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn exec_batched_am(&self,
        ame: Arc<ActiveMessageEngine>,
        src: usize,
        team_hash: u64,
        func_id: AmId,
        batch_id: usize,
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>,
        data_slice: &[u8],
    ){
        // println!("execing batch_id {:?}",batch_id);
        let mut index = 0;
        let mut results: Arc<Mutex<(HashMap<usize,LamellarReturn>,usize)>>= Arc::new(Mutex::new((HashMap::new(),0)));
        let  req_cnt = Arc::new(AtomicUsize::new(0));
        let  exec_cnt = Arc::new(AtomicUsize::new(0));
        let  processed = Arc::new(AtomicBool::new(false));
        let mut req_id =0;
        while index < data_slice.len(){
            req_cnt.fetch_add(1,Ordering::SeqCst);
            let func = AMS_EXECS.get(&(func_id)).unwrap()(&data_slice[index..]);
            index += func.serialized_size();
            let world = world.clone();
            let team = team.clone();
            let lamellae = lamellae.clone();
            let req_cnt = req_cnt.clone();
            let exec_cnt = exec_cnt.clone();
            let results = results.clone();
            let processed = processed.clone();
            let ame = ame.clone();
            self.scheduler.submit_task(async move {
                let lam_return = func.exec( team.world_pe, team.num_world_pes, false , world.clone(), team.clone()).await;
                // match &lam_return{
                //     LamellarReturn::Unit => println!("unit"),
                //     LamellarReturn::LocalData(_) => println!("local data"),
                //     LamellarReturn::LocalAm(_) => println!("local am"),
                //     LamellarReturn::RemoteData(_) => println!("remote data"),
                //     LamellarReturn::RemoteAm(_) => println!("remote am"),
                // }
                
                let (num_rets,num_unit_rets) = {
                    let mut entry = results.lock();
                    if let LamellarReturn::Unit  = &lam_return{
                        entry.1  += 1;
                    }
                    entry.0.insert(req_id,lam_return);
                    (entry.0.len(),entry.1)
                };
                let my_cnt = exec_cnt.fetch_add(1, Ordering::SeqCst) + 1;
                while my_cnt == req_cnt.load(Ordering::SeqCst){
                    if  processed.load(Ordering::SeqCst) == true {
                        if my_cnt == req_cnt.load(Ordering::SeqCst) {    
                                                    
                            if num_rets == num_unit_rets { //every result was a unit --- we probably can determine this from the function...
                                let  req_data = NewReqData{
                                    src: team.world_pe ,
                                    dst: Some(src),
                                    cmd: ExecType::Am(Cmd::BatchedUnitReturn),
                                    id: batch_id, //for this case where every result is a unit return we only submit a single message and the ids are generated automatically.
                                    batch_id: Some(batch_id),
                                    func:  LamellarFunc::None,
                                    lamellae: lamellae,
                                    world: world,
                                    team: team,
                                    team_hash: team_hash,
                                }; 
                                ame.process_msg_new(req_data, None).await;
                            }
                            else{
                                
                                join_all({
                                    let mut entry = results.lock();
                                    let mut msgs = vec![];
                                    for (req_id,lam_result) in entry.0.drain(){
                                        match lam_result{
                                            LamellarReturn::Unit =>{  
                                                panic!{"should not be the case that unit returns are mixed with data/am returns for batched am"}
                                            }
                                            LamellarReturn::RemoteData(d) => {
                                                let req_data = NewReqData{
                                                    src: team.world_pe,
                                                    dst: Some(src),
                                                    cmd: ExecType::Am(Cmd::BatchedDataReturn),
                                                    id: req_id,
                                                    batch_id: Some(batch_id),
                                                    func:  LamellarFunc::Result(d),
                                                    lamellae: lamellae.clone(),
                                                    world: world.clone(),
                                                    team: team.clone(),
                                                    team_hash: team_hash,
                                                };
                                                msgs.push(ame.process_msg_new(req_data, None));
                                            },
                                            LamellarReturn::RemoteAm(am) => {
                                                // println!("returnin remote am");
                                                let req_data = NewReqData{
                                                    src: team.world_pe,
                                                    dst: Some(src),
                                                    cmd: ExecType::Am(Cmd::BatchedAmReturn),
                                                    id: req_id,
                                                    batch_id: Some(batch_id),
                                                    func:  LamellarFunc::Am(am),
                                                    lamellae: lamellae.clone(),
                                                    world: world.clone(),
                                                    team: team.clone(),
                                                    team_hash: team_hash,
                                                };
                                                msgs.push(ame.process_msg_new(req_data, None));
                                            },
                                            _ => {
                                                panic!{"not handled yet"};
                                            }
                                        }
                                    }
                                    msgs
                                }).await;
                            }
                            
                        }
                        break;
                    }
                    async_std::task::yield_now().await;
                }
            });
            req_id +=1;
        }
        processed.store(true,Ordering::SeqCst);
    }

    fn process_am_return(&self,
        msg: Msg, 
        ser_data: SerializedData, 
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>) {
        if let Some(header) = ser_data.deserialize_header(){
            let data_slice = ser_data.data_as_bytes();
            self.exec_batched_return_am(msg.src as usize,header.team_hash,-header.id,0,lamellae,world,team,data_slice);
        }
    }

    fn process_batched_am_return(&self,
        msg: Msg, 
        ser_data: SerializedData, 
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>) {
        if let Some(header) = ser_data.deserialize_header(){
            let data_slice = ser_data.data_as_bytes();
            self.exec_batched_return_am(msg.src as usize,header.team_hash,-header.id,msg.req_id,lamellae,world,team,data_slice);
        }
    }
    
    fn exec_batched_return_am(&self,
        src: usize,
        team_hash: u64,
        func_id: AmId,
        batch_id: usize,
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>,
        data_slice: &[u8],
    ){
        // println!("execing return am batch_id {:?} func_id {:?}",batch_id,func_id);
        let mut index = 0;
        let serialized_size =  crate::serialized_size(&0usize);
        let cnt = if let Some(reqs) =self.txed_ams.lock().get_mut(&batch_id){
            let mut reqs = reqs.lock();
            // for (b_id,r_id) in reqs.iter(){
            //     println!("{:?} {:?}",b_id,r_id);
            // }
            while index < data_slice.len(){
                // println!("index {:?} len {:?}",index,data_slice.len());
                let batch_req_id: usize = crate::deserialize(&data_slice[index..(index+serialized_size)]).unwrap();                 
                index+=serialized_size;
                let func = AMS_EXECS.get(&(func_id)).unwrap()(&data_slice[index..]);
                index += func.serialized_size();
                let req_id =reqs.remove(&batch_req_id).expect("id not found");
                let world = world.clone();
                let team = team.clone();
                let lamellae = lamellae.clone();
                // let ame = ame.clone();
                self.scheduler.submit_task(async move {
                    let req_data = Arc::new(NewReqData{
                        src: src,
                        dst: Some(team.world_pe),
                        cmd: ExecType::Am(Cmd::Exec),
                        id: req_id,
                        batch_id: Some(batch_id),
                        func:  LamellarFunc::Am(func),
                        lamellae: lamellae,
                        world: world,
                        team: team,
                        team_hash: team_hash,
                    });
                    RegisteredActiveMessages::exec_local(
                        req_data
                    ).await;
                });                              
            }
            reqs.len()
        }else if batch_id ==0 { //not part a an original batch
            while index < data_slice.len(){
                // println!("index {:?} len {:?}",index,data_slice.len());
                let req_id: usize = crate::deserialize(&data_slice[index..(index+serialized_size)]).unwrap();                 
                index+=serialized_size;
                let func = AMS_EXECS.get(&(func_id)).unwrap()(&data_slice[index..]);
                index += func.serialized_size();
                let world = world.clone();
                let team = team.clone();
                let lamellae = lamellae.clone();
                // let ame = ame.clone();
                self.scheduler.submit_task(async move {
                    let req_data = Arc::new(NewReqData{
                        src: src,
                        dst: Some(team.world_pe),
                        cmd: ExecType::Am(Cmd::Exec),
                        id: req_id,
                        batch_id: Some(batch_id),
                        func:  LamellarFunc::Am(func),
                        lamellae: lamellae,
                        world: world,
                        team: team,
                        team_hash: team_hash,
                    });
                    RegisteredActiveMessages::exec_local(
                        req_data
                    ).await;
                }); 
            }
            1
        }
        else{
            println!("batch id {:?} doesnt exist",batch_id);
            1
        };
        if cnt == 0{
            self.txed_ams.lock().remove(&batch_id);
        }
    }

    fn process_batched_unit_return(&self, batch_id: usize, src: u16){
        // println!("processing returns {:?}",batch_id);
        let reqs = self.txed_ams.lock().remove(&batch_id);
        
        if let Some(reqs) = reqs{
            let reqs = reqs.lock();
            
            for (_,req_id) in reqs.iter(){
                // println!("completed req {:?}",req_id);
                ActiveMessageEngine::send_data_to_user_handle(*req_id,src,InternalResult::Unit);
            }
        }
        else{
            panic!("batch id {:?} not found",batch_id);
        }
    }

    fn process_unit_return(&self, src: u16, data_slice: &[u8]){

        // println!("processing returns {:?}",batch_id); 
        let mut index=0;
        let serialized_size = crate::serialized_size(&0usize);
        // println!("data_slice {:?}",data_slice);
        while index < data_slice.len(){
            let req_id: usize = crate::deserialize(&data_slice[index..(index+serialized_size)]).unwrap();
            // println!("completed req {:?}",req_id);
            ActiveMessageEngine::send_data_to_user_handle(req_id,src ,InternalResult::Unit);
            index+=serialized_size;
        }
    }

    fn process_data_return(&self, msg: Msg, ser_data: SerializedData){
        // println!("processing returns {:?}",batch_id);
        let data_slice=ser_data.data_as_bytes();  
        let mut index=0;
        let serialized_size = crate::serialized_size(&(0usize,0usize));
        // println!("data_slice {:?}",data_slice);
        while index < data_slice.len(){
            let (req_id,data_size): (usize,usize) = crate::deserialize(&data_slice[index..(index+serialized_size)]).unwrap();
            index+=serialized_size;
            let sub_data = ser_data.sub_data(index,index+data_size);
            ActiveMessageEngine::send_data_to_user_handle(req_id,msg.src,InternalResult::Remote(sub_data));
            index+=data_size;
        }
    }

    fn process_batched_data_return(&self,batch_id: usize, src: u16,  ser_data: SerializedData){

        let data_slice=ser_data.data_as_bytes();
        let mut index=0;
        // println!("data_slice {:?}",data_slice);
        let serialized_size = crate::serialized_size(&(0usize,0usize));
        let cnt = if let Some(reqs) =self.txed_ams.lock().get_mut(&batch_id){
            let mut reqs = reqs.lock();
            while index < data_slice.len(){
                // println!("index {:?} len {:?} ss {:?}",index,data_slice.len(),serialized_size);
                let (batch_req_id,data_size): (usize,usize) = crate::deserialize(&data_slice[index..(index+serialized_size)]).unwrap();
                // println!("batch_req_id data_size {:?}  {:?}",batch_req_id,data_size);
                index+=serialized_size;
                let sub_data = ser_data.sub_data(index,index+data_size);
                index+=data_size;
                let req_id =reqs.remove(&batch_req_id).expect("id not found");
                // println!("batch_req_id req_id data_size {:?} {:?} {:?}",batch_req_id,req_id,data_size);
                ActiveMessageEngine::send_data_to_user_handle(req_id,src,InternalResult::Remote(sub_data));
                                
            }
            reqs.len()
        }else{
            0
        };
        if cnt == 0{
            self.txed_ams.lock().remove(&batch_id);
        }
    }

    pub(crate) async fn process_batched_am(&self, //process_am
        ame: Arc<ActiveMessageEngine>,
        cmd: Cmd,
        msg: Msg,
        ser_data: SerializedData,
        lamellae: Arc<Lamellae>,
        world: Arc<LamellarTeamRT>,
        team: Arc<LamellarTeamRT>) {
        match cmd{
            Cmd::BatchedMsg => self.exec_batched_msg(ame,msg,ser_data,lamellae,world,team),
            Cmd::Exec => self.exec_single_msg(ame, msg,ser_data,lamellae,world,team,false).await,            
            Cmd::BatchedDataReturn => self.process_batched_data_return(msg.req_id,msg.src,ser_data),
            Cmd::DataReturn => self.process_data_return(msg,ser_data),
            Cmd::BatchedAmReturn => self.process_batched_am_return(msg,ser_data,lamellae,world,team),
            Cmd::AmReturn => self.process_am_return(msg,ser_data,lamellae,world,team),
            
            _ => println!("unhandled cmd {:?}",msg.cmd)
        }
    }
}
