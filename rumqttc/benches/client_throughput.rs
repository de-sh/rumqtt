/// Benchmark: async publish throughput and per-message heap allocations.
///
/// # Throughput (`publish_qos0`)
///
/// Measures how fast `AsyncClient::publish()` can enqueue messages onto the
/// request channel when the eventloop is draining them.
///
/// # Heap allocations (`heap_alloc`)
///
/// Uses dhat as the global allocator to count every heap allocation during a
/// fixed 10 000-message run.  The dhat `Profiler` is scoped tightly around
/// the publish loop, so criterion setup and warmup allocations are excluded.
///
/// Run:
///   cargo bench -p rumqttc --bench client_throughput -- heap_alloc
///
/// Output: `dhat-heap-<scenario>.json` in the current directory.
/// Open at: https://nnethercote.github.io/dh_view/dh_view.html
///
/// Key signal: look for call stacks with exactly 10 000 blocks — those are
/// per-message allocations.  1–5 block entries are one-time setup costs.
///
/// # Design
///
/// A single TCP connection is reused across all benchmark iterations. Opening
/// a fresh connection per iteration would exhaust the ~16 k ephemeral port
/// range before the measurement phase ends.
///
/// The loopback broker runs as a tokio task on the **same** single-threaded
/// runtime as the driver and publisher. It drains TCP immediately whenever
/// the driver yields — no cross-thread wakeup, no scheduling jitter — making
/// TCP effectively a blackhole and shifting the bottleneck to the eventloop.
///
/// # Running
///
///   cargo bench -p rumqttc --bench client_throughput
///
/// # Flamegraph profiling
///
/// Uses samply (macOS system profiler, no Xcode required); install once:
///   cargo install samply
///
///   CARGO_PROFILE_BENCH_DEBUG=2 CARGO_PROFILE_BENCH_STRIP=none \
///   CARGO_PROFILE_BENCH_LTO=false \
///   cargo bench -p rumqttc --bench client_throughput --no-run
///
///   samply record ./target/release/deps/client_throughput-<hash> --bench
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::{os::unix::io::AsRawFd, time::Duration};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    runtime::Builder as RuntimeBuilder,
    sync::oneshot,
    task,
};

const HEAP_MSGS: u64 = 10_000;

/// Minimal loopback broker: accepts one connection, bumps socket buffers to
/// 4 MB so the driver can always write without blocking (TCP as a blackhole),
/// sends ConnAck, then drains until the client disconnects (EOF).
async fn loopback_broker(listener: TcpListener) {
    let (stream, _) = listener.accept().await.unwrap();

    let raw = stream.as_raw_fd();
    let buf_size: libc::c_int = 4 * 1024 * 1024;
    unsafe {
        libc::setsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &buf_size as *const _ as *const libc::c_void,
            std::mem::size_of_val(&buf_size) as libc::socklen_t,
        );
        libc::setsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &buf_size as *const _ as *const libc::c_void,
            std::mem::size_of_val(&buf_size) as libc::socklen_t,
        );
    }

    let mut stream = stream;
    let mut buf = [0u8; 256];
    stream.read(&mut buf).await.unwrap();
    stream.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();

    let mut drain = [0u8; 65536];
    loop {
        match stream.read(&mut drain).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Establish a loopback connection and return (client, driver_handle, broker_handle).
/// The driver task polls the eventloop continuously; the broker drains TCP.
/// All three run on the given single-threaded runtime.
fn setup_loopback(
    rt: &tokio::runtime::Runtime,
) -> (
    AsyncClient,
    task::JoinHandle<()>,
    task::JoinHandle<()>,
) {
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let broker = task::spawn(loopback_broker(listener));

        let mut opts = MqttOptions::new("bench", "127.0.0.1", port);
        opts.set_inflight(100);
        let (client, mut eventloop) = AsyncClient::new(opts, 256);

        let (conn_tx, conn_rx) = oneshot::channel::<()>();
        let driver = task::spawn(async move {
            let mut conn_tx = Some(conn_tx);
            loop {
                let Ok(event) = eventloop.poll().await else {
                    break;
                };
                if let Event::Incoming(Packet::ConnAck(_)) = event {
                    if let Some(tx) = conn_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }
        });

        conn_rx.await.unwrap();
        (client, driver, broker)
    })
}

fn teardown_loopback(
    rt: &tokio::runtime::Runtime,
    client: AsyncClient,
    driver: task::JoinHandle<()>,
    broker: task::JoinHandle<()>,
) {
    rt.block_on(async {
        drop(client);
        driver.abort();
        let _ = driver.await;
        let _ = broker.await;
    });
}

fn client_throughput(c: &mut Criterion) {
    let rt = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (client, driver, broker) = setup_loopback(&rt);

    let mut group = c.benchmark_group("publish_qos0");
    group.measurement_time(Duration::from_secs(15));

    for payload_size in [32usize, 100, 1024] {
        let payload = vec![0xABu8; payload_size];
        group.throughput(Throughput::Elements(1000));
        group.bench_with_input(
            BenchmarkId::new("payload_bytes", payload_size),
            &payload_size,
            |b, &_size| {
                b.to_async(&rt).iter(|| {
                    let c = client.clone();
                    let p = payload.clone();
                    async move {
                        for _ in 0..1000 {
                            c.publish("bench/topic", QoS::AtMostOnce, false, p.clone())
                                .await
                                .unwrap();
                        }
                    }
                });
            },
        );
    }

    group.finish();
    teardown_loopback(&rt, client, driver, broker);
}

/// Heap allocation benchmark.
///
/// Runs a fixed `HEAP_MSGS`-message QoS-0 loop with a `dhat::Profiler` scoped
/// tightly around the publish loop.  Criterion's warmup and setup allocations
/// happen *before* the Profiler is created and are excluded from the JSON.
///
/// Each scenario writes its own `dhat-heap-<scenario>.json`.  Run a single
/// scenario with:
///   cargo bench -p rumqttc --bench client_throughput -- "heap_alloc/<name>"
///
/// Scenarios:
///   fresh  — new Vec<u8> payload per message (worst case, isolates user-space cost)
///   reused — pre-built Bytes payload cloned per message (library floor)
fn heap_alloc(c: &mut Criterion) {
    let rt = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (client, driver, broker) = setup_loopback(&rt);

    let mut group = c.benchmark_group("heap_alloc");
    // One sample: the dhat JSON is the artifact; timing here is secondary.
    group
        .warm_up_time(Duration::from_millis(1))
        .sample_size(10);
    group.throughput(Throughput::Elements(HEAP_MSGS));

    // Scenario: fresh — new Vec<u8> allocation per message.
    // This is the worst-case baseline: every publish allocates a fresh payload.
    // The dhat output will show a 100B × 10 000 site for the payload vec — that
    // is user-space overhead, not library cost.
    group.bench_function("fresh", |b| {
        b.iter_custom(|_iters| {
            let profiler = dhat::Profiler::builder()
                .file_name("dhat-heap-fresh.json")
                .build();
            let start = std::time::Instant::now();
            rt.block_on(async {
                for _ in 0..HEAP_MSGS {
                    client
                        .publish("bench/topic", QoS::AtMostOnce, false, vec![0xABu8; 100])
                        .await
                        .unwrap();
                }
            });
            let elapsed = start.elapsed();
            drop(profiler);
            elapsed
        });
    });

    // Scenario: reused — arc-backed Bytes cloned per message.
    // Pre-building the payload as Bytes avoids the per-message Vec alloc.
    // What remains is the library floor: topic String + any internal bookkeeping.
    // Expect ~1–2 allocations per message from rumqttc itself.
    group.bench_function("reused", |b| {
        use bytes::Bytes;
        let payload = Bytes::from(vec![0xABu8; 100]);
        b.iter_custom(|_iters| {
            let profiler = dhat::Profiler::builder()
                .file_name("dhat-heap-reused.json")
                .build();
            let start = std::time::Instant::now();
            rt.block_on(async {
                for _ in 0..HEAP_MSGS {
                    client
                        .publish("bench/topic", QoS::AtMostOnce, false, payload.clone())
                        .await
                        .unwrap();
                }
            });
            let elapsed = start.elapsed();
            drop(profiler);
            elapsed
        });
    });

    group.finish();
    teardown_loopback(&rt, client, driver, broker);
}

criterion_group!(benches, client_throughput, heap_alloc);
criterion_main!(benches);
