//! An experiment in how to safely represent and use the PDI (Process Data Image).
//!
//! At time of writing, requires EL2004, EL2004 and EL1004 in that order to function correctly due
//! to a pile of hard-coding.

use async_ctrlc::CtrlC;
use ethercrab::{
    coe::SubIndex, error::Error, std::tx_rx_task, Client, PduLoop, SlaveGroup, SlaveState,
};
use futures_lite::FutureExt;
use smol::LocalExecutor;
use std::{sync::Arc, time::Duration};

#[cfg(target_os = "windows")]
// ASRock NIC
// const INTERFACE: &str = "TODO";
// // USB NIC
// const INTERFACE: &str = "\\Device\\NPF_{DCEDC919-0A20-47A2-9788-FC57D0169EDB}";
// Lenovo USB-C NIC
const INTERFACE: &str = "\\Device\\NPF_{CC0908D5-3CB8-46D6-B8A2-575D0578008D}";
// Silver USB NIC
// const INTERFACE: &str = "\\Device\\NPF_{CC0908D5-3CB8-46D6-B8A2-575D0578008D}";
#[cfg(not(target_os = "windows"))]
const INTERFACE: &str = "eth1";

const MAX_SLAVES: usize = 16;
const MAX_PDU_DATA: usize = 1100;
const MAX_FRAMES: usize = 16;
const PDI_LEN: usize = 64;

static PDU_LOOP: PduLoop<MAX_FRAMES, MAX_PDU_DATA, smol::Timer> = PduLoop::new();

async fn main_inner(ex: &LocalExecutor<'static>) -> Result<(), Error> {
    log::info!("Starting SDO demo...");

    let client = Arc::new(Client::<MAX_FRAMES, MAX_PDU_DATA, smol::Timer>::new(
        &PDU_LOOP,
    ));

    ex.spawn(tx_rx_task(INTERFACE, &client).unwrap()).detach();

    // let num_slaves = client.num_slaves();

    let groups =
        [SlaveGroup::<MAX_SLAVES, PDI_LEN, MAX_FRAMES, MAX_PDU_DATA, _>::new(Box::new(|slave| {
            Box::pin(async {
                // --- Reads ---

                // // Name
                // dbg!(slave
                //     .read_sdo::<heapless::String<64>>(0x1008, SdoAccess::Index(0))
                //     .await
                //     .unwrap());

                // // Software version. For AKD, this should equal "M_01-20-00-003"
                // dbg!(slave
                //     .read_sdo::<heapless::String<64>>(0x100a, SdoAccess::Index(0))
                //     .await
                //     .unwrap());

                // --- Writes ---

                log::info!("Found {}", slave.name());

                // AKD config
                if slave.name() == "AKD" {
                    slave.write_sdo(0x1c12, 0u8, SubIndex::Index(0)).await?;
                    slave
                        .write_sdo(0x1c12, 0x1720u16, SubIndex::Index(1))
                        .await?;
                    slave.write_sdo(0x1c12, 0x01u8, SubIndex::Index(0)).await?;

                    // Must set both read AND write SDOs for AKD otherwise it times out going into OP
                    slave.write_sdo(0x1c13, 0u8, SubIndex::Index(0)).await?;
                    slave
                        .write_sdo(0x1c13, 0x1B22u16, SubIndex::Index(1))
                        .await?;
                    slave.write_sdo(0x1c13, 0x01u8, SubIndex::Index(0)).await?;
                }

                if slave.name() == "ELP-EC400S" {
                    slave.write_sdo(0x1c12, 0u8, SubIndex::Index(0)).await?;
                    slave
                        .write_sdo(0x1c12, 0x1601u16, SubIndex::Index(1))
                        .await?;
                    slave.write_sdo(0x1c12, 0x01u8, SubIndex::Index(0)).await?;

                    slave.write_sdo(0x1c13, 0u8, SubIndex::Index(0)).await?;
                    slave
                        .write_sdo(0x1c13, 0x1A00u16, SubIndex::Index(1))
                        .await?;
                    slave.write_sdo(0x1c13, 0x01u8, SubIndex::Index(0)).await?;
                }

                Ok(())
            })
        })); 1];

    let mut groups = client
        .init(groups, |groups, slave| {
            // All slaves MUST end up in a group or they'll remain uninitialised
            groups[0].push(slave).expect("Too many slaves");

            // TODO: Return a group key so the user has to put the slave somewhere
        })
        .await
        .expect("Init");

    // log::info!("Discovered {num_slaves} slaves");

    // NOTE: Valid outputs must be provided before moving into operational state
    log::debug!("Moving slaves to OP...");

    client
        .request_slave_state(SlaveState::Op)
        .await
        .expect("OP");

    log::info!("Slaves moved to OP state");

    let group = groups.get_mut(0).expect("No group!");

    log::info!("Group has {} slaves", group.slaves().len());

    loop {
        let (i, _o) = group.io(0).unwrap();

        println!("{:?}", i.unwrap());

        async_io::Timer::after(Duration::from_millis(100)).await;
    }
}

fn main() -> Result<(), Error> {
    env_logger::init();
    let local_ex = LocalExecutor::new();

    let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");

    futures_lite::future::block_on(
        local_ex.run(ctrlc.race(async { main_inner(&local_ex).await.unwrap() })),
    );

    Ok(())
}
