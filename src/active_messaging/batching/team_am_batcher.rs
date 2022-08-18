use crate::active_messaging::registered_active_message::*;
use crate::active_messaging::*;
use crate::lamellae::comm::AllocError;
use crate::lamellae::{Des, Lamellae, LamellaeAM, LamellaeRDMA, Ser, SerializeHeader};
use crate::lamellar_arch::LamellarArchRT;
use crate::{LamellarTeam, LamellarWorld};
use batching::*;

use async_trait::async_trait;

const MAX_BATCH_SIZE: usize = 1_000_000;

lazy_static! {
    static ref TEAM_HEADER_LEN: usize =
        crate::serialized_size::<TeamHeader>(&Default::default(), false);
    static ref BATCHED_AM_HEADER_LEN: usize =
        crate::serialized_size::<BatchedAmHeader>(&Default::default(), false);
    static ref REQ_ID_LEN: usize = crate::serialized_size::<ReqId>(&Default::default(), false);
}

type TeamId = usize;
type AmIdMap = HashMap<AmId, Vec<(ReqMetaData, LamellarArcAm, usize)>>;
type TeamMap = HashMap<TeamId, AmIdMap>;

#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
struct TeamHeader {
    team_id: TeamId,
    am_batch_cnts: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
struct BatchedAmHeader {
    am_id: AmId,
    am_cnt: usize,
    cmd: Cmd,
}

#[derive(Clone, Debug)]
struct TeamAmBatcherInner {
    batch: Arc<Mutex<(TeamMap, TeamMap, Vec<(ReqMetaData, LamellarData, usize)>)>>,
    size: Arc<AtomicUsize>,
    pe: Option<usize>,
}

impl TeamAmBatcherInner {
    //#[tracing::instrument(skip_all)]
    fn new(pe: Option<usize>) -> TeamAmBatcherInner {
        TeamAmBatcherInner {
            batch: Arc::new(Mutex::new((HashMap::new(), HashMap::new(), Vec::new()))),
            size: Arc::new(AtomicUsize::new(0)),
            pe: pe,
        }
    }

    #[tracing::instrument(skip_all)]
    fn add_am_to_batch(
        &self,
        req_data: ReqMetaData,
        am: LamellarArcAm,
        id: AmId,
        size: usize,
        batch: &mut TeamMap,
    ) {
        let mut temp_size = 0;
        let team_batch = batch
            .entry(req_data.team_addr)
            .or_insert_with(|| HashMap::new());
        if team_batch.len() == 0 {
            temp_size += *TEAM_HEADER_LEN;
            //println!(
            //     "adding team header {} {} {}",
            //     temp_size,
            //     *TEAM_HEADER_LEN,
            //     self.size.load(Ordering::SeqCst)
            // );
        }
        let am_batch = team_batch.entry(id).or_insert_with(|| Vec::new());
        if am_batch.len() == 0 {
            temp_size += *BATCHED_AM_HEADER_LEN;
            //println!(
            //     "adding batched header {} {} {}",
            //     temp_size,
            //     *BATCHED_AM_HEADER_LEN,
            //     self.size.load(Ordering::SeqCst)
            // );
        }
        am_batch.push((req_data, am, size));
        temp_size += size + *REQ_ID_LEN;
        //println!(
        //     "adding req_id + size header {} {} {} {}",
        //     temp_size,
        //     *REQ_ID_LEN,
        //     size,
        //     self.size.load(Ordering::SeqCst)
        // );
        self.size.fetch_add(temp_size, Ordering::SeqCst);
        //println!("updated size: {:?}", self.size.load(Ordering::SeqCst));
    }

    //#[tracing::instrument(skip_all)]
    fn add_am(&self, req_data: ReqMetaData, data: LamellarData, size: usize) -> bool {
        match data {
            LamellarData::Am(am, id) => {
                let mut batch = self.batch.lock();
                let first = self.size.load(Ordering::SeqCst) == 0;
                self.add_am_to_batch(req_data, am, id, size, &mut batch.0);
                first
            }
            LamellarData::Return(am, id) => {
                let mut batch = self.batch.lock();
                let first = self.size.load(Ordering::SeqCst) == 0;
                self.add_am_to_batch(req_data, am, id, size, &mut batch.1);
                first
            }
            _ => {
                panic!("unexpected data type");
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    fn add_non_am(&self, req_data: ReqMetaData, data: LamellarData, size: usize) -> bool {
        let mut batch = self.batch.lock();
        let size = size + *CMD_LEN;
        batch.2.push((req_data, data, size));
        let first = self.size.fetch_add(size, Ordering::SeqCst) == 0;
        first
    }

    //#[tracing::instrument(skip_all)]
    fn swap(
        &self,
    ) -> (
        TeamMap,
        TeamMap,
        Vec<(ReqMetaData, LamellarData, usize)>,
        usize,
    ) {
        let mut batch = self.batch.lock();
        let mut new_batch = (HashMap::new(), HashMap::new(), Vec::new());
        std::mem::swap(&mut batch.0, &mut new_batch.0);
        std::mem::swap(&mut batch.1, &mut new_batch.1);
        std::mem::swap(&mut batch.2, &mut new_batch.2);
        let size = self.size.load(Ordering::SeqCst);
        self.size.store(0, Ordering::SeqCst);
        (new_batch.0, new_batch.1, new_batch.2, size)
    }
}

#[derive(Debug)]
pub(crate) struct TeamAmBatcher {
    batched_ams: Arc<Vec<TeamAmBatcherInner>>,
    stall_mark: Arc<AtomicUsize>,
}

#[async_trait]
impl Batcher for TeamAmBatcher {
    //#[tracing::instrument(skip_all)]
    fn add_remote_am_to_batch(
        &self,
        req_data: ReqMetaData,
        am: LamellarArcAm,
        am_id: AmId,
        am_size: usize,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        stall_mark: usize,
    ) {
        let batch = match req_data.dst {
            Some(dst) => self.batched_ams[dst].clone(),
            None => self.batched_ams.last().unwrap().clone(),
        };
        if stall_mark == 0 {
            self.stall_mark.fetch_add(1, Ordering::Relaxed);
        }
        if batch.add_am(req_data.clone(), LamellarData::Am(am, am_id), am_size) {
            // it true this means we need to create a tx task.
            self.create_tx_task(
                batch,
                stall_mark,
                scheduler,
                req_data.lamellae.clone(),
                req_data.team.arch.clone(),
                req_data.team.world_pe,
            );
        }
    }

    //#[tracing::instrument(skip_all)]
    fn add_return_am_to_batch(
        &self,
        req_data: ReqMetaData,
        am: LamellarArcAm,
        am_id: AmId,
        am_size: usize,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        stall_mark: usize,
    ) {
        let batch = match req_data.dst {
            Some(dst) => self.batched_ams[dst].clone(),
            None => self.batched_ams.last().unwrap().clone(),
        };
        if stall_mark == 0 {
            self.stall_mark.fetch_add(1, Ordering::Relaxed);
        }
        if batch.add_am(req_data.clone(), LamellarData::Return(am, am_id), am_size) {
            // it true this means we need to create a tx task.
            self.create_tx_task(
                batch,
                stall_mark,
                scheduler,
                req_data.lamellae.clone(),
                req_data.team.arch.clone(),
                req_data.team.world_pe,
            );
        }
    }

    //#[tracing::instrument(skip_all)]
    fn add_data_am_to_batch(
        &self,
        req_data: ReqMetaData,
        data: LamellarResultArc,
        data_size: usize,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        stall_mark: usize,
    ) {
        let batch = match req_data.dst {
            Some(dst) => self.batched_ams[dst].clone(),
            None => self.batched_ams.last().unwrap().clone(),
        };
        if stall_mark == 0 {
            self.stall_mark.fetch_add(1, Ordering::Relaxed);
        }
        if batch.add_non_am(
            req_data.clone(),
            LamellarData::Data(data),
            data_size + *DATA_HEADER_LEN,
        ) {
            // it true this means we need to create a tx task.
            self.create_tx_task(
                batch,
                stall_mark,
                scheduler,
                req_data.lamellae.clone(),
                req_data.team.arch.clone(),
                req_data.team.world_pe,
            );
        }
    }

    //#[tracing::instrument(skip_all)]
    fn add_unit_am_to_batch(
        &self,
        req_data: ReqMetaData,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        stall_mark: usize,
    ) {
        let batch = match req_data.dst {
            Some(dst) => self.batched_ams[dst].clone(),
            None => self.batched_ams.last().unwrap().clone(),
        };
        if stall_mark == 0 {
            self.stall_mark.fetch_add(1, Ordering::Relaxed);
        }
        if batch.add_non_am(req_data.clone(), LamellarData::Unit, *UNIT_HEADER_LEN) {
            // it true this means we need to create a tx task.
            self.create_tx_task(
                batch,
                stall_mark,
                scheduler,
                req_data.lamellae.clone(),
                req_data.team.arch.clone(),
                req_data.team.world_pe,
            );
        }
    }

    //#[tracing::instrument(skip_all)]
    async fn exec_batched_msg(
        &self,
        msg: Msg,
        ser_data: SerializedData,
        lamellae: Arc<Lamellae>,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        ame: &RegisteredActiveMessages,
    ) {
        let data = ser_data.data_as_bytes();
        let mut i = 0;

        while i < data.len() {
            let cmd: Cmd = crate::deserialize(&data[i..i + *CMD_LEN], false).unwrap();
            //println!("cmd {:?} i: {} len: {}", cmd, i, data.len());
            i += *CMD_LEN;
            match cmd {
                Cmd::Am | Cmd::ReturnAm => {
                    panic!("should not encounter individual am cmds in TeamAmBatcher")
                }
                Cmd::Data => ame.exec_data_am(&msg, data, &mut i, &ser_data).await,
                Cmd::Unit => ame.exec_unit_am(&msg, data, &mut i).await,
                Cmd::BatchedMsg => {
                    self.exec_batched_am(&msg, data, &mut i, &lamellae, scheduler, &ame);
                }
            }
        }
    }
}

impl TeamAmBatcher {
    //#[tracing::instrument(skip_all)]
    pub(crate) fn new(num_pes: usize, stall_mark: Arc<AtomicUsize>) -> TeamAmBatcher {
        let mut batched_ams = Vec::new();
        for pe in 0..num_pes {
            batched_ams.push(TeamAmBatcherInner::new(Some(pe)));
        }
        batched_ams.push(TeamAmBatcherInner::new(None));
        TeamAmBatcher {
            batched_ams: Arc::new(batched_ams),
            stall_mark: stall_mark,
        }
    }
    //#[tracing::instrument(skip_all)]
    fn create_tx_task(
        &self,
        batch: TeamAmBatcherInner,
        mut stall_mark: usize,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        lamellae: Arc<Lamellae>,
        arch: Arc<LamellarArchRT>,
        my_pe: usize,
    ) {
        let cur_stall_mark = self.stall_mark.clone();
        scheduler.submit_task(async move {
            while stall_mark != cur_stall_mark.load(Ordering::SeqCst)
                && batch.size.load(Ordering::SeqCst) < MAX_BATCH_SIZE
            {
                stall_mark = cur_stall_mark.load(Ordering::Relaxed);
                async_std::task::yield_now().await;
            }
            let (am_batch, return_am_batch, non_am_batch, mut size) = batch.swap();
            if am_batch.len() > 0 {
                size += *CMD_LEN
            }
            if return_am_batch.len() > 0 {
                size += *CMD_LEN
            }
            let header = TeamAmBatcher::create_header(my_pe);
            let data_buf = TeamAmBatcher::create_data_buf(header, size, &lamellae).await;
            let data_slice = data_buf.data_as_bytes();

            //println!("total batch size: {}", size);
            let mut i = 0;
            TeamAmBatcher::serialize_am_batch(am_batch, data_slice, &mut i, Cmd::Am);
            TeamAmBatcher::serialize_am_batch(return_am_batch, data_slice, &mut i, Cmd::ReturnAm);
            TeamAmBatcher::serialize_non_am_batch(non_am_batch, data_slice, &mut i);
            lamellae.send_to_pes_async(batch.pe, arch, data_buf).await;
        });
    }

    //#[tracing::instrument(skip_all)]
    fn serialize_am_batch(am_batch: TeamMap, data_slice: &mut [u8], i: &mut usize, cmd: Cmd) {
        if am_batch.len() > 0 {
            crate::serialize_into(&mut data_slice[*i..*i + *CMD_LEN], &Cmd::BatchedMsg, false)
                .unwrap();
            *i += *CMD_LEN;
            //println!("after batch cmd -- i: {}", i);
            for (team_id, am_map) in am_batch {
                let team_header = TeamHeader {
                    team_id: team_id,
                    am_batch_cnts: am_map.len(),
                };
                crate::serialize_into(
                    &mut data_slice[*i..*i + *TEAM_HEADER_LEN],
                    &team_header,
                    false,
                )
                .unwrap();
                *i += *TEAM_HEADER_LEN;
                //println!("after team header -- i: {}", i);

                for (am_id, ams) in am_map {
                    let batched_am_header = BatchedAmHeader {
                        am_id: am_id,
                        am_cnt: ams.len(),
                        cmd: cmd,
                    };
                    crate::serialize_into(
                        &mut data_slice[*i..*i + *BATCHED_AM_HEADER_LEN],
                        &batched_am_header,
                        false,
                    )
                    .unwrap();
                    *i += *BATCHED_AM_HEADER_LEN;
                    //println!("after batched header -- i: {}", i);
                    for (req_data, am, size) in ams {
                        TeamAmBatcher::serialize_am(req_data, am, am_id, size, data_slice, i);
                    }
                }
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    fn serialize_non_am_batch(
        non_am_batch: Vec<(ReqMetaData, LamellarData, usize)>,
        data_slice: &mut [u8],
        i: &mut usize,
    ) {
        for (req_data, data, size) in non_am_batch {
            match data {
                LamellarData::Am(_, _) | LamellarData::Return(_, _) => {
                    panic!("should not have non am batch with am or return data");
                }
                LamellarData::Data(data) => {
                    TeamAmBatcher::serialize_data(req_data, data, size, data_slice, i);
                }
                LamellarData::Unit => {
                    TeamAmBatcher::serialize_unit(req_data, data_slice, i);
                }
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    fn serialize_am(
        req_data: ReqMetaData,
        am: LamellarArcAm,
        am_id: AmId,
        am_size: usize,
        data_buf: &mut [u8],
        i: &mut usize,
    ) {
        crate::serialize_into(&mut data_buf[*i..*i + *REQ_ID_LEN], &req_data.id, false).unwrap();
        *i += *REQ_ID_LEN;
        //println!("after req id -- i: {}", i);
        //println!("am size: {}", am_size);
        // let am_size = am_size - (*REQ_ID_LEN);
        //println!(
        //     "am size: {} {} {}",
        //     am_size,
        //     am.serialized_size(),
        //     data_buf.len()
        // );

        let darc_ser_cnt = match req_data.dst {
            Some(_) => 1,
            None => {
                match req_data.team.team_pe_id() {
                    Ok(_) => req_data.team.num_pes() - 1, //we dont send an am to ourself here
                    Err(_) => req_data.team.num_pes(), //this means we have a handle to a team but are not in the team
                }
            }
        };
        am.ser(darc_ser_cnt);
        am.serialize_into(&mut data_buf[*i..*i + am_size]);
        *i += am_size;
    }

    //#[tracing::instrument(skip_all)]
    fn serialize_data(
        req_data: ReqMetaData,
        data: LamellarResultArc,
        data_size: usize,
        data_buf: &mut [u8],
        i: &mut usize,
    ) {
        crate::serialize_into(&mut data_buf[*i..*i + *CMD_LEN], &Cmd::Data, false).unwrap();
        *i += *CMD_LEN;
        let data_size = data_size - (*CMD_LEN + *DATA_HEADER_LEN);
        let data_header = DataHeader {
            size: data_size,
            req_id: req_data.id,
        };
        crate::serialize_into(
            &mut data_buf[*i..*i + *DATA_HEADER_LEN],
            &data_header,
            false,
        )
        .unwrap();
        *i += *DATA_HEADER_LEN;
        data.serialize_into(&mut data_buf[*i..*i + data_size]);
        *i += data_size;
    }

    //#[tracing::instrument(skip_all)]
    fn serialize_unit(req_data: ReqMetaData, data_buf: &mut [u8], i: &mut usize) {
        crate::serialize_into(&mut data_buf[*i..*i + *CMD_LEN], &Cmd::Unit, false).unwrap();
        *i += *CMD_LEN;

        let unit_header = UnitHeader {
            req_id: req_data.id,
        };
        crate::serialize_into(
            &mut data_buf[*i..*i + *UNIT_HEADER_LEN],
            &unit_header,
            false,
        )
        .unwrap();
        *i += *UNIT_HEADER_LEN;
    }

    //#[tracing::instrument(skip_all)]
    fn create_header(src: usize) -> SerializeHeader {
        let msg = Msg {
            src: src as u16,
            cmd: Cmd::BatchedMsg,
        };
        SerializeHeader { msg: msg }
    }

    //#[tracing::instrument(skip_all)]
    async fn create_data_buf(
        header: SerializeHeader,
        size: usize,
        lamellae: &Arc<Lamellae>,
    ) -> SerializedData {
        let header = Some(header);
        let mut data = lamellae.serialize_header(header.clone(), size);
        while let Err(err) = data {
            async_std::task::yield_now().await;
            match err.downcast_ref::<AllocError>() {
                Some(AllocError::OutOfMemoryError(_)) => {
                    lamellae.alloc_pool(size * 2);
                }
                _ => panic!("unhanlded error!! {:?}", err),
            }
            data = lamellae.serialize_header(header.clone(), size);
        }
        data.unwrap()
    }

    //#[tracing::instrument(skip_all)]
    fn exec_batched_am(
        &self,
        msg: &Msg,
        data: &[u8],
        i: &mut usize,
        lamellae: &Arc<Lamellae>,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        ame: &RegisteredActiveMessages,
    ) {
        let team_header: TeamHeader =
            crate::deserialize(&data[*i..*i + *TEAM_HEADER_LEN], false).unwrap();
        //println!("team header: {:?}", team_header);
        *i += *TEAM_HEADER_LEN;
        let (team, world) =
            ame.get_team_and_world(msg.src as usize, team_header.team_id, &lamellae);

        for _am_batchs in 0..team_header.am_batch_cnts {
            let batched_am_header: BatchedAmHeader =
                crate::deserialize(&data[*i..*i + *BATCHED_AM_HEADER_LEN], false).unwrap();
            //println!("batched am header: {:?}", batched_am_header);
            *i += *BATCHED_AM_HEADER_LEN;
            for _am in 0..batched_am_header.am_cnt {
                //println!("am cmd: {:?}", batched_am_header.cmd);
                match batched_am_header.cmd {
                    Cmd::Am => self.exec_am(
                        msg,
                        data,
                        i,
                        lamellae,
                        scheduler,
                        ame,
                        batched_am_header.am_id,
                        world.clone(),
                        team.clone(),
                    ),
                    Cmd::ReturnAm => self.exec_return_am(
                        msg,
                        data,
                        i,
                        lamellae,
                        scheduler,
                        ame,
                        batched_am_header.am_id,
                        world.clone(),
                        team.clone(),
                    ),
                    _ => panic!("unhandled cmd"),
                }
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    fn exec_am(
        &self,
        msg: &Msg,
        data: &[u8],
        i: &mut usize,
        lamellae: &Arc<Lamellae>,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        ame: &RegisteredActiveMessages,
        am_id: AmId,
        world: Arc<LamellarTeam>,
        team: Arc<LamellarTeam>,
    ) {
        let req_id = crate::deserialize(&data[*i..*i + *REQ_ID_LEN], false).unwrap();
        *i += *REQ_ID_LEN;
        let am = AMS_EXECS.get(&am_id).unwrap()(&data[*i..], team.team.team_pe);
        *i += am.serialized_size();

        let req_data = ReqMetaData {
            src: team.team.world_pe,
            dst: Some(msg.src as usize),
            id: req_id,
            lamellae: lamellae.clone(),
            world: world.team.clone(),
            team: team.team.clone(),
            team_addr: team.team.remote_ptr_addr,
        };
        scheduler.submit_task(async move {
            let am = match am
                .exec(
                    team.team.world_pe,
                    team.team.num_world_pes,
                    false,
                    world.clone(),
                    team.clone(),
                )
                .await
            {
                LamellarReturn::Unit => Am::Unit(req_data),
                LamellarReturn::RemoteData(data) => Am::Data(req_data, data),
                LamellarReturn::RemoteAm(am) => Am::Return(req_data, am),
                LamellarReturn::LocalData(_) | LamellarReturn::LocalAm(_) => {
                    panic!("Should not be returning local data or AM from remote  am");
                }
            };
            ame.process_msg(am, scheduler, 0).await;
        });
    }

    //#[tracing::instrument(skip_all)]
    fn exec_return_am(
        &self,
        msg: &Msg,
        data: &[u8],
        i: &mut usize,
        lamellae: &Arc<Lamellae>,
        scheduler: &(impl SchedulerQueue + Sync + std::fmt::Debug),
        ame: &RegisteredActiveMessages,
        am_id: AmId,
        world: Arc<LamellarTeam>,
        team: Arc<LamellarTeam>,
    ) {
        let req_id = crate::deserialize(&data[*i..*i + *REQ_ID_LEN], false).unwrap();
        *i += *REQ_ID_LEN;
        let am = AMS_EXECS.get(&am_id).unwrap()(&data[*i..], team.team.team_pe);
        *i += am.serialized_size();

        let req_data = ReqMetaData {
            src: msg.src as usize,
            dst: Some(team.team.world_pe),
            id: req_id,
            lamellae: lamellae.clone(),
            world: world.team.clone(),
            team: team.team.clone(),
            team_addr: team.team.remote_ptr_addr,
        };
        scheduler.submit_task(ame.exec_local_am(req_data, am.as_local(), world, team));
    }
}
