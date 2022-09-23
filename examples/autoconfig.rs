//! Read slave configuration from EEPROM and automatically apply it.

use async_ctrlc::CtrlC;
use ethercrab::al_status::AlState;
use ethercrab::client::Client;
use ethercrab::error::{Error, PduError};
use ethercrab::fmmu::Fmmu;
use ethercrab::pdu::CheckWorkingCounter;
use ethercrab::register::RegisterAddress;
use ethercrab::std::tx_rx_task;
use ethercrab::sync_manager_channel::{Direction, OperationMode, SyncManagerChannel};
use futures_lite::FutureExt;
use futures_lite::StreamExt;
use packed_struct::PackedStruct;
use smol::LocalExecutor;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_os = "windows")]
// ASRock NIC
// const INTERFACE: &str = "TODO";
// USB NIC
const INTERFACE: &str = "\\Device\\NPF_{DCEDC919-0A20-47A2-9788-FC57D0169EDB}";
// Silver USB NIC
// const INTERFACE: &str = "\\Device\\NPF_{CC0908D5-3CB8-46D6-B8A2-575D0578008D}";
#[cfg(not(target_os = "windows"))]
const INTERFACE: &str = "eth1";

async fn main_inner(ex: &LocalExecutor<'static>) -> Result<(), Error> {
    let client = Arc::new(Client::<16, 16, 16, smol::Timer>::new());

    ex.spawn(tx_rx_task(INTERFACE, &client).unwrap()).detach();

    let (_res, num_slaves) = client.brd::<u8>(RegisterAddress::Type).await.unwrap();

    log::info!("Discovered {num_slaves} slaves");

    client.init().await.expect("Init");

    {
        for idx in 0..num_slaves {
            let slave = client.slave_by_index(idx)?;

            slave.configure_from_eeprom().await?;
        }

        // // TODO: Read this from EEPROM
        // let write_sm = SyncManagerChannel {
        //     physical_start_address: 0x0f00,
        //     length: 1,
        //     control: ethercrab::sync_manager_channel::Control {
        //         operation_mode: OperationMode::Buffered,
        //         direction: Direction::MasterWrite,
        //         watchdog_enable: true,
        //         ..Default::default()
        //     },
        //     status: ethercrab::sync_manager_channel::Status::default(),
        //     enable: ethercrab::sync_manager_channel::Enable {
        //         enable: true,
        //         ..Default::default()
        //     },
        // };

        // client
        //     .fpwr(0x1001, RegisterAddress::Sm0, write_sm.pack().unwrap())
        //     .await
        //     .unwrap()
        //     .wkc(1, "SM0")
        //     .unwrap();
    }

    {
        // TODO: Read this from EEPROM
        let write_fmmu = Fmmu {
            logical_start_address: 0x00000000,
            length: 0x01,
            logical_start_bit: 0x00,
            logical_end_bit: 0x03,
            physical_start_address: 0x0f00,
            physical_start_bit: 0x0,
            read_enable: false,
            write_enable: true,
            enable: true,
            reserved_1: 0,
            reserved_2: 0,
        };

        client
            .fpwr(0x1001, RegisterAddress::Fmmu0, write_fmmu.pack().unwrap())
            .await
            .unwrap()
            .wkc(1, "FMMU0")
            .unwrap();
    }

    client
        .request_slave_state(AlState::PreOp)
        .await
        .expect(&format!("Slave PRE-OP"));

    client
        .request_slave_state(AlState::SafeOp)
        .await
        .expect(&format!("Slave SAFE-OP"));

    client
        .request_slave_state(AlState::Op)
        .await
        .expect(&format!("Slave OP"));

    let value = Rc::new(RefCell::new(0x00u8));

    let value2 = value.clone();
    let client2 = client.clone();

    // PD TX task (no RX because EL2004 is WO)
    ex.spawn(async move {
        // Cycle time
        let mut interval = async_io::Timer::interval(Duration::from_millis(2));

        while let Some(_) = interval.next().await {
            let v: u8 = *value2.borrow();

            client2.lwr(0u32, v).await.expect("Bad write");
        }
    })
    .detach();

    // Blink frequency
    let mut interval = async_io::Timer::interval(Duration::from_millis(250));

    while let Some(_) = interval.next().await {
        if *value.borrow() == 0 {
            *value.borrow_mut() = 0b0000_0010;
        } else {
            *value.borrow_mut() = 0;
        }
    }
    Ok(())
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
