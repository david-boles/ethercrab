use crate::{
    command::Command, error::PduError, pdu::Pdu, slave::Slave, timer_factory::TimerFactory,
    ETHERCAT_ETHERTYPE, MASTER_ADDR,
};
use core::{
    cell::{BorrowMutError, RefCell, RefMut},
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
    task::{Poll, Waker},
};
use futures::future::{select, Either};
use smoltcp::wire::EthernetFrame;

#[derive(Debug, PartialEq)]
pub enum RequestState {
    Created,
    Waiting,
    Done,
}

// TODO: Use atomic_refcell crate
pub struct ClientInternals<
    const MAX_FRAMES: usize,
    const MAX_PDU_DATA: usize,
    const MAX_SLAVES: usize,
    TIMEOUT,
> {
    wakers: RefCell<[Option<Waker>; MAX_FRAMES]>,
    frames: RefCell<[Option<(RequestState, Pdu<MAX_PDU_DATA>)>; MAX_FRAMES]>,
    send_waker: RefCell<Option<Waker>>,
    idx: AtomicU8,
    _timeout: PhantomData<TIMEOUT>,
    // TODO: un-pub
    pub slaves: RefCell<heapless::Vec<Slave, MAX_SLAVES>>,
}

unsafe impl<const MAX_FRAMES: usize, const MAX_PDU_DATA: usize, const MAX_SLAVES: usize, TIMEOUT>
    Sync for ClientInternals<MAX_FRAMES, MAX_PDU_DATA, MAX_SLAVES, TIMEOUT>
{
}

impl<const MAX_FRAMES: usize, const MAX_PDU_DATA: usize, const MAX_SLAVES: usize, TIMEOUT>
    ClientInternals<MAX_FRAMES, MAX_PDU_DATA, MAX_SLAVES, TIMEOUT>
where
    TIMEOUT: TimerFactory,
{
    pub fn new() -> Self {
        // MSRV: Make `N` a `u8` when `generic_const_exprs` is stablised
        assert!(
            MAX_FRAMES < u8::MAX.into(),
            "Packet indexes are u8s, so cache array cannot be any bigger than u8::MAX"
        );

        Self {
            wakers: RefCell::new([(); MAX_FRAMES].map(|_| None)),
            frames: RefCell::new([(); MAX_FRAMES].map(|_| None)),
            send_waker: RefCell::new(None),
            idx: AtomicU8::new(0),
            slaves: RefCell::new(heapless::Vec::new()),
            _timeout: PhantomData,
        }
    }

    pub fn set_send_waker(&self, waker: &Waker) {
        if self.send_waker.borrow().is_none() {
            self.send_waker.borrow_mut().replace(waker.clone());
        }
    }

    pub fn frames_mut(
        &self,
    ) -> Result<RefMut<'_, [Option<(RequestState, Pdu<MAX_PDU_DATA>)>; MAX_FRAMES]>, BorrowMutError>
    {
        self.frames.try_borrow_mut()
    }

    pub async fn pdu(
        &self,
        command: Command,
        data: &[u8],
        data_length: u16,
    ) -> Result<Pdu<MAX_PDU_DATA>, PduError> {
        // braces to ensure we don't hold the refcell across awaits
        let idx = {
            // TODO: Confirm ordering
            let idx = self.idx.fetch_add(1, Ordering::Release) % MAX_FRAMES as u8;

            // We're receiving too fast or the receive buffer isn't long enough
            if self.frames.borrow()[usize::from(idx)].is_some() {
                return Err(PduError::IndexInUse);
            }

            let mut pdu = Pdu::<MAX_PDU_DATA>::new(command, data_length, idx);

            pdu.data = data.try_into().map_err(|_| PduError::TooLong)?;

            self.frames.borrow_mut()[usize::from(idx)] = Some((RequestState::Created, pdu));

            // println!("TX waker? {:?}", self.send_waker);

            if let Some(waker) = &*self.send_waker.borrow() {
                waker.wake_by_ref()
            }

            usize::from(idx)
        };

        // MSRV: Use core::future::poll_fn when `future_poll_fn ` is stabilised
        let res = futures_lite::future::poll_fn(|ctx| {
            // TODO: Races
            let mut frames = self.frames.borrow_mut();

            let frame = frames[usize::from(idx)].take();

            let res = match frame {
                Some((RequestState::Done, pdu)) => Poll::Ready(pdu),
                // Not ready yet, put the request back.
                // TODO: This is dumb, we just want a reference
                Some(state) => {
                    frames[usize::from(idx)] = Some(state);
                    Poll::Pending
                }
                _ => Poll::Pending,
            };

            self.wakers.borrow_mut()[usize::from(idx)] = Some(ctx.waker().clone());

            res
        });

        // TODO: Configurable timeout
        let timeout = TIMEOUT::timer(core::time::Duration::from_micros(30_000));

        let res = match select(res, timeout).await {
            Either::Left((res, _timeout)) => res,
            Either::Right((_timeout, _res)) => return Err(PduError::Timeout),
        };

        Ok(res)
    }

    // TODO: Return a result if index is out of bounds, or we don't have a waiting packet
    pub fn parse_response_ethernet_packet(&self, raw_packet: &[u8]) {
        let raw_packet = EthernetFrame::new_unchecked(raw_packet);

        // Look for EtherCAT packets whilst ignoring broadcast packets sent from self
        if raw_packet.ethertype() != ETHERCAT_ETHERTYPE || raw_packet.src_addr() == MASTER_ADDR {
            return ();
        }

        let (_rest, pdu) = Pdu::<MAX_PDU_DATA>::from_ethernet_payload::<nom::error::Error<&[u8]>>(
            &raw_packet.payload(),
        )
        .expect("Packet parse");

        let idx = pdu.index;

        let waker = self.wakers.borrow_mut()[usize::from(idx)].take();

        // Frame is ready; tell everyone about it
        if let Some(waker) = waker {
            // TODO: Borrow races
            if let Some((state, existing_pdu)) = self.frames.borrow_mut()[usize::from(idx)].as_mut()
            {
                pdu.is_response_to(existing_pdu).unwrap();

                *state = RequestState::Done;
                *existing_pdu = pdu
            } else {
                panic!("No waiting frame for response");
            }

            waker.wake()
        }
    }
}
