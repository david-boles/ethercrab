use core::future::poll_fn;
use core::task::Poll;
use criterion::{criterion_group, criterion_main, Bencher, Criterion, Throughput};
use ethercrab::{Client, ClientConfig, Command, PduStorage, Timeouts};
use futures_lite::FutureExt;
use std::pin::pin;

const DATA: [u8; 8] = [0x11u8, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xdd];

fn do_bench(b: &mut Bencher) {
    const FRAME_OVERHEAD: usize = 28;

    // 1 frame, up to 128 bytes payload
    let storage = PduStorage::<1, { PduStorage::element_size(128) }>::new();

    let (mut tx, mut rx, pdu_loop) = storage.try_split().unwrap();

    let client = Client::new(pdu_loop, Timeouts::default(), ClientConfig::default());

    let mut written_packet = [0u8; { FRAME_OVERHEAD + DATA.len() }];

    b.iter(|| {
        //  --- Prepare frame

        let mut frame_fut = pin!(Command::fpwr(0x5678, 0x1234).send_receive::<()>(&client, &DATA));

        let frame_fut = poll_fn(|ctx| {
            let _ = frame_fut.poll(ctx);

            // --- Send frame

            if let Some(frame) = tx.next_sendable_frame() {
                frame
                    .send_blocking(|bytes| {
                        written_packet.copy_from_slice(bytes);

                        Ok(bytes.len())
                    })
                    .expect("TX");

                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });

        let _ = cassette::block_on(frame_fut);

        // --- Receive frame

        let _ = rx.receive_frame(&written_packet).expect("RX");
    })
}

pub fn tx_rx(c: &mut Criterion) {
    let mut group = c.benchmark_group("pdu_loop");

    group.throughput(Throughput::Elements(1));

    group.bench_function("elements", do_bench);

    group.throughput(Throughput::Bytes(DATA.len() as u64));

    group.bench_function("payload bytes", do_bench);

    group.finish();
}

criterion_group!(pdu_loop, tx_rx);
criterion_main!(pdu_loop);
