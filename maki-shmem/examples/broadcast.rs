//! A stand-in maki producer: creates this process's session ring and
//! broadcasts `Hello`, `MakiStatus` and `ChatLine` until killed, so a
//! consumer (haunt) has a live feed to attach to.
//!
//! ```sh
//! cargo run -p maki-shmem --example broadcast
//! ```

use maki_shmem::{ChatLine, Hello, MakiStatus, ring, text};

const BROADCAST_PERIOD_MS: u64 = 500;

fn main() {
    let pid = std::process::id();
    let path = ring::ring_path(pid);
    let out = ring::Ring::create(&path, ring::MAKI_NODE_ID, 0).expect("create session ring");
    let mut producer = ring::Producer::new(&out);
    out.update_status(ring::Status::Running);
    println!("maki-shmem broadcast: {path}");

    let mut hello = Hello {
        magic: 0x4D414B49,
        version: 1,
        role: 0,
        reserved_7: 0,
        pid,
        reserved_12: [0; 4],
        timestamp_ns: 0,
        sender_name: [0; 64],
        reserved_88: [0; 8],
    };
    text::encode(&mut hello.sender_name, "maki-broadcast");
    hello.timestamp_ns = ring::monotonic_nanos() as u64;
    producer.publish(1, &hello);
    producer.flush();

    let mut tick: u32 = 0;
    loop {
        let mut status = MakiStatus {
            pid,
            state: (tick % 3) as u8 + 1,
            flags: 0,
            reserved_6: 0,
            timestamp_ns: ring::monotonic_nanos() as u64,
            context_used: 1000 * tick,
            context_limit: 200_000,
            cost_usd: tick as f64 * 0.01,
            session_id: [0; 32],
            session_title: [0; 64],
            last_tool: [0; 32],
            mac: [0; 16],
            reserved_176: [0; 16],
        };
        text::encode(&mut status.session_id, "example-session");
        text::encode(&mut status.session_title, "broadcast example");
        text::encode(&mut status.last_tool, "bash");
        producer.publish(5, &status);

        if tick.is_multiple_of(4) {
            let mut line = ChatLine {
                timestamp_ns: status.timestamp_ns,
                sender_name: [0; 32],
                text: [0; 456],
            };
            text::encode(&mut line.sender_name, "maki");
            text::encode(&mut line.text, "still broadcasting");
            producer.publish(2, &line);
        }

        producer.flush();
        out.update_heartbeat(ring::monotonic_nanos());
        tick += 1;
        std::thread::sleep(std::time::Duration::from_millis(BROADCAST_PERIOD_MS));
    }
}
