//! The BlitRing container over the generated layout: one single-writer
//! producer, up to [`RING_REGISTRY_LINES`] registered readers, in shared
//! memory. Ported from blitgen-zig's `demo/ring/blitring.rs`; the protocol is
//! described in `demo/ring/README.md`.
//!
//! Publication is three stores: payload, identity word, then `seq` with a
//! release store — that last store is the entire protocol. A consumer
//! acquire-loads `seq` and everything before the release store is visible.
//! The registry is presence-only: a reader stamps its own line at attach and
//! never writes again, so every cache line in the mapping has exactly one
//! writer. There is no producer-side gating: a reader slower than a full lap
//! observes the overwrite as a sequence gap and resyncs from the producer's
//! cursor.
//!
//! maki is one producer per session ring ([`ring_path`]); consumers such as
//! haunt attach to every ring [`session_ring_paths`] finds.

use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering};

use crate::schema;

pub const MAGIC: [u8; 4] = *b"BRNG";
pub const FORMAT_VERSION: u16 = 2;
pub const FIRST_SEQUENCE: u64 = 1;
/// The identity word a single-slot record carries: template id in the low 16
/// bits, `span == 1`, `span_index == 0`, `reserved == 0`.
pub const SPAN_ONE: u64 = 65536;
pub const SPAN_SHIFT: u8 = 16;
/// How often the producer publishes its cursor; a power of two, so the test
/// is an AND. Consumers read that line once per batch.
pub const CURSOR_BATCH: u64 = 1024;
/// The file is rounded up to a whole huge page so tmpfs can back it with one.
const HUGE_PAGE: usize = 2 * 1024 * 1024;

pub const CAPACITY: usize = schema::Ring512x2048::RING_CAPACITY;
pub const STRIDE: usize = schema::Ring512x2048::RING_SLOT_STRIDE;
pub const HEADER_BYTES: usize = schema::Ring512x2048::RING_HEADER_BYTES;
pub const SLOT_HEADER_SIZE: usize = schema::RingSlotHeader::LENGTH as usize;
pub const BODY_BYTES: usize = STRIDE - SLOT_HEADER_SIZE;
pub const MAX_CONSUMERS: usize = schema::Ring512x2048::RING_REGISTRY_LINES;
pub const LINE_SIZE: u16 = schema::Ring512x2048::RING_LINE_SIZE as u16;
pub const SLOT_COUNT_POW2: u8 = schema::Ring512x2048::RING_SLOT_COUNT_POW2;
pub const SLOT_SIZE_POW2: u8 = schema::Ring512x2048::RING_SLOT_SIZE_POW2;
const MASK: u64 = CAPACITY as u64 - 1;
const IDENTITY_OFFSET: usize = schema::RingSlotHeader::OFFSET_template_id;
pub const RING_BYTES: usize = HEADER_BYTES + CAPACITY * STRIDE;
pub const MAPPED_BYTES: usize = RING_BYTES.next_multiple_of(HUGE_PAGE);

/// The layout identity this build stamps into a ring header and refuses a
/// mismatch of: the leading eight bytes of the ring composition's digest.
pub const SCHEMA_HASH: u64 = schema::BLIT_CONTAINER_SHA256_PREFIX_Ring512x2048;

pub type Slot = schema::RingSlot512;

const _: () = assert!(std::mem::size_of::<Slot>() == STRIDE);
const _: () = assert!(CAPACITY.is_power_of_two());
const _: () = assert!(1usize << SLOT_COUNT_POW2 == CAPACITY);
const _: () = assert!(1usize << SLOT_SIZE_POW2 == STRIDE);
const _: () = assert!(std::mem::size_of::<schema::RingControl>() <= LINE_SIZE as usize);
const _: () = assert!(std::mem::size_of::<schema::RingConsumerSlot>() == LINE_SIZE as usize);

/// The maki producer's node id in consumer registries.
pub const MAKI_NODE_ID: u8 = 1;
/// A dashboard consumer's node id.
pub const HAUNT_NODE_ID: u8 = 2;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RingError {
    #[error("not a ring file")]
    NotABlitRing,
    #[error("unsupported ring format version")]
    UnsupportedFormatVersion,
    #[error("ring was built against a different layout")]
    SchemaMismatch,
    #[error("ring offers weaker cache-line isolation")]
    WeakerIsolation,
    #[error("ring geometry does not match this build")]
    GeometryMismatch,
    #[error("producer has not finished initialising the header")]
    NotInitialised,
    #[error("ring file is truncated")]
    Truncated,
    #[error("mapping the ring file failed")]
    MapFailed,
    #[error("opening the ring file failed")]
    FileOpenFailed,
}

/// The producer's operational state, as recorded in the header.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    Unknown = 0,
    Starting = 1,
    Running = 2,
    Draining = 3,
    Stopped = 4,
}

impl Status {
    pub fn from_wire(value: i32) -> Status {
        match value {
            1 => Status::Starting,
            2 => Status::Running,
            3 => Status::Draining,
            4 => Status::Stopped,
            _ => Status::Unknown,
        }
    }
}

/// Where a reader begins: at the producer's cursor, or at the oldest
/// sequence still resident.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StartAt {
    Head,
    Earliest,
}

/// Where session rings live: tmpfs on Linux, `/tmp` elsewhere.
pub fn ring_dir() -> String {
    if cfg!(target_os = "linux") {
        "/dev/shm".to_string()
    } else {
        "/tmp".to_string()
    }
}

/// The ring file one maki process owns, named by pid.
pub fn ring_path(pid: u32) -> String {
    format!("{}/maki-{}.brng", ring_dir(), pid)
}

/// Every session ring a consumer can attach to, sorted by name.
pub fn session_ring_paths() -> Vec<String> {
    let dir = ring_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("maki-") && name.ends_with(".brng")
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

/// A mapped ring. `Send`/`Sync` by hand: the mapping is shared memory and
/// every operation is an atomic or a behind-the-gate store.
pub struct Ring {
    memory: *mut u8,
    length: usize,
    slots: *mut Slot,
    pub record_target: u32,
    created: Option<CString>,
}

unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Creates the ring and initialises its header, built under a temporary
    /// name and renamed into place so a consumer never maps a half-written
    /// file. The file is unlinked when this Ring is dropped.
    pub fn create(path: &str, sender_id: u8, record_target: u32) -> Result<Ring, RingError> {
        let pending = CString::new(format!("{path}.tmp")).map_err(|_| RingError::FileOpenFailed)?;
        let final_path = CString::new(path).map_err(|_| RingError::FileOpenFailed)?;

        let fd = unsafe {
            libc::open(pending.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC, 0o600)
        };
        if fd < 0 {
            return Err(RingError::FileOpenFailed);
        }
        if unsafe { libc::ftruncate(fd, MAPPED_BYTES as libc::off_t) } != 0 {
            unsafe { libc::close(fd) };
            return Err(RingError::FileOpenFailed);
        }

        let memory = Ring::map(fd, MAPPED_BYTES);
        unsafe { libc::close(fd) };
        let memory = memory?;

        let ring = Ring {
            memory,
            length: MAPPED_BYTES,
            slots: unsafe { memory.add(HEADER_BYTES) } as *mut Slot,
            record_target,
            created: Some(final_path),
        };

        unsafe {
            std::ptr::write_bytes(memory, 0, HEADER_BYTES);
            let header = &mut *(memory as *mut schema::RingHeader);
            header.slot_size_pow2 = SLOT_SIZE_POW2;
            header.slot_count_pow2 = SLOT_COUNT_POW2;
            header.sender_id = sender_id;
            header.start_time = monotonic_nanos();
            header.current_status = Status::Starting as i32;
            header.producer_pid = std::process::id() as i32;
            header.magic = MAGIC;
            header.format_version = FORMAT_VERSION;
            header.line_size = LINE_SIZE;
            header.header_bytes = HEADER_BYTES as u32;
            header.schema_hash = SCHEMA_HASH;
            header.record_target = record_target;
            header.producer.sequence = FIRST_SEQUENCE;
        }
        // Published last, so anything that reads a 1 sees a fully
        // initialised header behind it.
        ring.init_flag().store(1, Ordering::Release);

        // The mapping survives the rename: it holds the inode, not the name.
        if unsafe { libc::rename(pending.as_ptr(), ring.created.as_ref().unwrap().as_ptr()) } != 0
        {
            unsafe { libc::unlink(pending.as_ptr()) };
            return Err(RingError::FileOpenFailed);
        }
        Ok(ring)
    }

    /// Adopts a ring another process created, refusing one this build cannot
    /// read.
    pub fn open(path: &str) -> Result<Ring, RingError> {
        let c_path = CString::new(path).map_err(|_| RingError::FileOpenFailed)?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            return Err(RingError::FileOpenFailed);
        }
        let length = unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
        if length < HEADER_BYTES as libc::off_t {
            unsafe { libc::close(fd) };
            return Err(RingError::Truncated);
        }
        let length = length as usize;

        let memory = Ring::map(fd, length);
        unsafe { libc::close(fd) };
        let memory = memory?;

        let header = unsafe { &*(memory as *const schema::RingHeader) };
        // The rename in `create` closes the init window; a producer
        // initialising an existing path does not have it closed.
        let init = unsafe { &*(memory as *const AtomicU8) };
        if init.load(Ordering::Acquire) == 0 {
            return Err(RingError::NotInitialised);
        }
        if header.magic != MAGIC {
            return Err(RingError::NotABlitRing);
        }
        if header.format_version != FORMAT_VERSION {
            return Err(RingError::UnsupportedFormatVersion);
        }
        if header.schema_hash != SCHEMA_HASH {
            return Err(RingError::SchemaMismatch);
        }
        if header.line_size < LINE_SIZE {
            return Err(RingError::WeakerIsolation);
        }
        // The check that licenses every constant above: past this point the
        // file's geometry is the declared ring's.
        if header.slot_count_pow2 != SLOT_COUNT_POW2
            || header.slot_size_pow2 != SLOT_SIZE_POW2
            || header.header_bytes as usize != HEADER_BYTES
        {
            return Err(RingError::GeometryMismatch);
        }
        if length < RING_BYTES {
            return Err(RingError::Truncated);
        }

        Ok(Ring {
            memory,
            length,
            slots: unsafe { memory.add(HEADER_BYTES) } as *mut Slot,
            record_target: header.record_target,
            created: None,
        })
    }

    /// Opens a ring that may not exist yet, polling until `timeout_ms`
    /// elapses. A consumer normally starts before the producer.
    pub fn attach(path: &str, timeout_ms: u64) -> Result<Ring, RingError> {
        let mut waited = 0u64;
        loop {
            match Ring::open(path) {
                Ok(ring) => return Ok(ring),
                Err(RingError::FileOpenFailed | RingError::NotInitialised) if waited < timeout_ms => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    waited += 5;
                }
                Err(other) => return Err(other),
            }
        }
    }

    fn init_flag(&self) -> &AtomicU8 {
        unsafe { &*(self.memory as *const AtomicU8) }
    }

    /// The header, as the generated record.
    pub fn header(&self) -> &schema::RingHeader {
        unsafe { &*(self.memory as *const schema::RingHeader) }
    }

    /// How many bytes are mapped; only `RING_BYTES` of it is ever addressed.
    pub fn mapped_len(&self) -> usize {
        self.length
    }

    fn consumer_line(&self, index: usize) -> *mut u8 {
        const AT: usize = std::mem::offset_of!(schema::RingHeader, consumers);
        unsafe { self.memory.add(AT + index * schema::RingConsumerSlot::LENGTH as usize) }
    }

    pub fn update_status(&self, status: Status) {
        const AT: usize = std::mem::offset_of!(schema::RingHeader, current_status);
        let cell = unsafe { &*(self.memory.add(AT) as *const AtomicI32) };
        cell.store(status as i32, Ordering::Release);
    }

    pub fn update_heartbeat(&self, nanos: i64) {
        const AT: usize = std::mem::offset_of!(schema::RingHeader, last_heartbeat);
        let cell = unsafe { &*(self.memory.add(AT) as *const AtomicU64) };
        cell.store(nanos as u64, Ordering::Release);
    }

    /// The producer's `(status, last heartbeat)` for liveness checks.
    pub fn peer_health(&self) -> (Status, i64) {
        const STATUS_AT: usize = std::mem::offset_of!(schema::RingHeader, current_status);
        const BEAT_AT: usize = std::mem::offset_of!(schema::RingHeader, last_heartbeat);
        let status = unsafe { &*(self.memory.add(STATUS_AT) as *const AtomicI32) };
        let beat = unsafe { &*(self.memory.add(BEAT_AT) as *const AtomicU64) };
        (
            Status::from_wire(status.load(Ordering::Acquire)),
            beat.load(Ordering::Acquire) as i64,
        )
    }

    /// Records that `node_id` reads this ring. A reader owns the line at its
    /// own node id — no claim counter, so registration is one release store
    /// and idempotent. Ids at or beyond [`MAX_CONSUMERS`] read normally but
    /// are not represented.
    pub fn register_consumer(&self, node_id: u8) {
        if node_id as usize >= MAX_CONSUMERS {
            return;
        }
        let line = self.consumer_line(node_id as usize);
        unsafe {
            *line.add(1) = node_id;
            (line.add(4) as *mut i32).write_unaligned(std::process::id() as i32);
        }
        let gate = unsafe { &*(line as *const AtomicU8) };
        gate.store(1, Ordering::Release);
    }

    /// The node ids currently registered as readers, in node-id order.
    pub fn registered_consumers(&self) -> Vec<u8> {
        let mut found = Vec::new();
        for index in 0..MAX_CONSUMERS {
            let line = self.consumer_line(index);
            let gate = unsafe { &*(line as *const AtomicU8) };
            if gate.load(Ordering::Acquire) == 1 {
                found.push(unsafe { *line.add(1) });
            }
        }
        found
    }

    fn published_cell(&self) -> &AtomicU64 {
        const AT: usize = std::mem::offset_of!(schema::RingHeader, producer)
            + std::mem::offset_of!(schema::RingControl, sequence);
        unsafe { &*(self.memory.add(AT) as *const AtomicU64) }
    }

    /// The producing node's identity.
    pub fn sender_id(&self) -> u8 {
        self.header().sender_id
    }

    /// The producer's published count of records. Read once per batch, never
    /// per record: the producer writes this line on every publish.
    #[inline]
    pub fn published(&self) -> u64 {
        self.published_cell().load(Ordering::Acquire)
    }

    #[inline]
    fn slot(&self, sequence: u64) -> *mut Slot {
        unsafe { self.slots.add((sequence & MASK) as usize) }
    }

    #[inline]
    fn slot_seq(slot: *mut Slot) -> &'static AtomicU64 {
        const AT: usize = std::mem::offset_of!(Slot, header)
            + std::mem::offset_of!(schema::RingSlotHeader, seq);
        const _: () = assert!(AT == 0);
        unsafe { &*((slot as *mut u8).add(AT) as *const AtomicU64) }
    }

    fn map(fd: i32, length: usize) -> Result<*mut u8, RingError> {
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if memory == libc::MAP_FAILED {
            return Err(RingError::MapFailed);
        }
        Ok(memory as *mut u8)
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.memory as *mut libc::c_void, self.length) };
        if let Some(path) = self.created.take() {
            unsafe { libc::unlink(path.as_ptr()) };
        }
    }
}

/// The single writer. One producer per ring.
pub struct Producer<'a> {
    ring: &'a Ring,
    /// The next sequence to publish, held here rather than re-read from the
    /// mapping.
    pub next: u64,
}

impl<'a> Producer<'a> {
    pub fn new(ring: &'a Ring) -> Producer<'a> {
        let published = ring.published();
        Producer {
            ring,
            next: published.max(FIRST_SEQUENCE),
        }
    }

    /// Publishes the true tail, so a reader sees where the producer actually
    /// got to rather than the last batch boundary.
    pub fn flush(&mut self) {
        self.ring.published_cell().store(self.next, Ordering::Release);
    }

    /// Publishes a record. Three stores, one of them ordered: payload, then
    /// identity, then `seq` with a release store — and that release store is
    /// the entire protocol.
    pub fn publish<T: Copy>(&mut self, msg_id: u16, record: &T) -> u64 {
        assert!(
            std::mem::size_of::<T>() <= BODY_BYTES,
            "record is larger than this ring's slot body"
        );

        let sequence = self.next;
        let slot = self.ring.slot(sequence);

        unsafe {
            std::ptr::copy_nonoverlapping(
                record as *const T as *const u8,
                (slot as *mut u8).add(SLOT_HEADER_SIZE),
                std::mem::size_of::<T>(),
            );
            let identity = (slot as *mut u8).add(IDENTITY_OFFSET) as *mut u64;
            identity.write(msg_id as u64 | SPAN_ONE);
        }

        Ring::slot_seq(slot).store(sequence, Ordering::Release);
        self.next = sequence + 1;

        if sequence & (CURSOR_BATCH - 1) == 0 {
            self.flush();
        }
        sequence
    }
}

/// What a poll returned.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Received { sequence: u64, msg_id: u16 },
    Waiting,
    /// The producer overwrote sequences before this reader read them; the
    /// reader has resynced to the oldest live slot.
    Lapped,
    /// The slot's identity word is not one this build can decode.
    Unsupported,
    Finished,
}

/// What a batched read hands each record to. `body` points into the slot
/// and is valid until the consumer advances.
pub trait RingVisitor {
    fn on_record(&mut self, sequence: u64, msg_id: u16, body: &[u8; BODY_BYTES], last_of_batch: bool);
}

/// A reader. Many may run at once, in any language, with no registration.
///
/// This is a broadcast ring with no producer-side gating: a consumer that
/// stalls longer than the ring takes to fill has the slot it is reading
/// overwritten underneath it, and observes that as a sequence gap on the next
/// slot. The mitigation is ring depth.
pub struct Consumer {
    pub next: u64,
    pub dropped: u64,
    pub laps: u64,
    pub unsupported: u64,
    record_target: u64,
}

impl Consumer {
    /// `StartAt::Head` joins the live stream and never replays;
    /// `StartAt::Earliest` reads the oldest sequence still resident.
    pub fn new(ring: &Ring, start: StartAt) -> Consumer {
        let next = match start {
            StartAt::Head => ring.published().max(FIRST_SEQUENCE),
            StartAt::Earliest => FIRST_SEQUENCE,
        };
        Consumer {
            next,
            dropped: 0,
            laps: 0,
            unsupported: 0,
            record_target: ring.record_target as u64,
        }
    }

    #[inline]
    fn body(slot: *mut Slot) -> &'static [u8; BODY_BYTES] {
        unsafe { &*((slot as *const u8).add(SLOT_HEADER_SIZE) as *const [u8; BODY_BYTES]) }
    }

    /// One record, or a reason there is not one. The hot path touches
    /// exactly one cache line: the slot's own.
    pub fn poll(&mut self, ring: &Ring) -> Poll {
        let wanted = self.next;
        let slot = ring.slot(wanted);
        let seq = Ring::slot_seq(slot).load(Ordering::Acquire);
        if seq != wanted {
            return self.not_ours(ring, seq, wanted);
        }
        self.next = wanted + 1;

        // `identity >> SPAN_SHIFT == 1` is the whole validity test: it holds
        // exactly when span == 1, span_index == 0 and reserved == 0.
        let identity = Consumer::identity_of(slot);
        if identity >> SPAN_SHIFT != 1 {
            self.unsupported += 1;
            return Poll::Unsupported;
        }
        Poll::Received { sequence: wanted, msg_id: identity as u16 }
    }

    #[inline]
    fn identity_of(slot: *mut Slot) -> u64 {
        unsafe { ((slot as *const u8).add(IDENTITY_OFFSET) as *const u64).read() }
    }

    /// The body of the record `poll` just returned.
    #[inline]
    pub fn last_body(&self, ring: &Ring) -> &[u8; BODY_BYTES] {
        Consumer::body(ring.slot(self.next - 1))
    }

    /// Reads everything published, handing each record to `visitor`. Returns
    /// the number of records drained; at most one resync happens per call.
    pub fn read_all<V: RingVisitor>(&mut self, ring: &Ring, visitor: &mut V) -> usize {
        let mut local = self.next;
        let mut slot = ring.slot(local);
        let mut seq = Ring::slot_seq(slot).load(Ordering::Acquire);
        if seq > local {
            self.not_ours(ring, seq, local);
            local = self.next;
            slot = ring.slot(local);
            seq = Ring::slot_seq(slot).load(Ordering::Acquire);
        }
        if seq != local {
            return 0;
        }

        // The cursor, once, and only for the end-of-batch hint.
        let committed_end = ring.published();

        let started = local;
        loop {
            let identity = Consumer::identity_of(slot);
            if identity >> SPAN_SHIFT == 1 {
                visitor.on_record(
                    local,
                    identity as u16,
                    Consumer::body(slot),
                    local + 1 >= committed_end,
                );
            } else {
                self.unsupported += 1;
            }

            local += 1;
            slot = ring.slot(local);
            seq = Ring::slot_seq(slot).load(Ordering::Acquire);
            if seq != local {
                break;
            }
        }

        self.next = local;
        if seq > local {
            self.not_ours(ring, seq, local);
        }
        (local - started) as usize
    }

    fn not_ours(&mut self, ring: &Ring, seq: u64, wanted: u64) -> Poll {
        if seq > wanted {
            // Resync from the producer's cursor, not from the sequence in
            // the slot just read: that one is only `wanted + k * capacity`,
            // so resuming from it advances one record per lap.
            let published = ring.published();
            let oldest = if published > CAPACITY as u64 {
                published - CAPACITY as u64 + 1
            } else {
                FIRST_SEQUENCE
            };
            let resume_at = if oldest > wanted { oldest } else { wanted + 1 };
            self.dropped += resume_at - wanted;
            self.laps += 1;
            self.next = resume_at;
            return Poll::Lapped;
        }
        if self.record_target != 0 && wanted > self.record_target {
            return Poll::Finished;
        }
        Poll::Waiting
    }
}

/// A monotonic timestamp in nanoseconds, comparable within a host.
pub fn monotonic_nanos() -> i64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec * 1_000_000_000 + now.tv_nsec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hello, MakiStatus};

    fn unique_path(tag: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}/maki-test-{}-{tag}.brng", ring_dir(), n)
    }

    fn maki_status(context_used: u32) -> MakiStatus {
        let mut status = MakiStatus {
            pid: std::process::id(),
            state: 1,
            flags: 0,
            reserved_6: 0,
            timestamp_ns: 42,
            context_used,
            context_limit: 200_000,
            cost_usd: 0.25,
            session_id: [0; 32],
            session_title: [0; 64],
            last_tool: [0; 32],
            mac: [0; 16],
            reserved_176: [0; 16],
        };
        crate::text::encode(&mut status.session_id, "abcd1234");
        crate::text::encode(&mut status.session_title, "ring test");
        crate::text::encode(&mut status.last_tool, "bash");
        status
    }

    struct Collector {
        records: Vec<(u16, Vec<u8>)>,
    }

    impl RingVisitor for Collector {
        fn on_record(&mut self, _sequence: u64, msg_id: u16, body: &[u8; BODY_BYTES], _last: bool) {
            self.records.push((msg_id, body.to_vec()));
        }
    }

    #[test]
    fn publish_receive_roundtrip() {
        let path = unique_path("roundtrip");
        let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
        assert_eq!(ring.header().magic, MAGIC);
        assert_eq!(ring.header().schema_hash, SCHEMA_HASH);
        assert_eq!(ring.sender_id(), MAKI_NODE_ID);

        let mut producer = Producer::new(&ring);
        let hello = Hello {
            magic: 0x4D414B49,
            version: 1,
            role: 0,
            reserved_7: 0,
            pid: std::process::id(),
            reserved_12: [0; 4],
            timestamp_ns: 1,
            sender_name: [0; 64],
            reserved_88: [0; 8],
        };
        producer.publish(1, &hello);
        let status = maki_status(1234);
        producer.publish(5, &status);
        producer.flush();
        assert_eq!(ring.published(), FIRST_SEQUENCE + 2);

        let reader = Ring::open(&path).expect("open");
        reader.register_consumer(HAUNT_NODE_ID);
        assert_eq!(reader.registered_consumers(), vec![HAUNT_NODE_ID]);
        let mut consumer = Consumer::new(&reader, StartAt::Earliest);

        let Poll::Received { sequence, msg_id } = consumer.poll(&reader) else {
            panic!("expected the hello record");
        };
        assert_eq!((sequence, msg_id), (FIRST_SEQUENCE, 1));
        let got = Hello::copy_from_bytes(consumer.last_body(&reader)).expect("hello body");
        assert_eq!(got.pid, hello.pid);

        let Poll::Received { msg_id, .. } = consumer.poll(&reader) else {
            panic!("expected the status record");
        };
        assert_eq!(msg_id, 5);
        let got = MakiStatus::copy_from_bytes(consumer.last_body(&reader)).expect("status body");
        assert_eq!(got.context_used, 1234);
        assert_eq!(crate::text::decode(&got.session_title), "ring test");
        assert_eq!(consumer.poll(&reader), Poll::Waiting);
    }

    #[test]
    fn read_all_drains_and_visits_in_order() {
        let path = unique_path("drain");
        let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
        let mut producer = Producer::new(&ring);
        for i in 0..10u32 {
            let status = maki_status(i);
            producer.publish(5, &status);
        }
        producer.flush();

        let reader = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&reader, StartAt::Earliest);
        let mut collector = Collector { records: Vec::new() };
        assert_eq!(consumer.read_all(&reader, &mut collector), 10);
        assert_eq!(collector.records.len(), 10);
        let first = MakiStatus::copy_from_bytes(&collector.records[0].1).expect("body");
        let last = MakiStatus::copy_from_bytes(&collector.records[9].1).expect("body");
        assert_eq!((first.context_used, last.context_used), (0, 9));
        assert!(collector.records.iter().all(|(id, _)| *id == 5));
    }

    #[test]
    fn lapped_consumer_resyncs_and_accounts() {
        let path = unique_path("lap");
        let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
        let mut producer = Producer::new(&ring);
        let total = (CAPACITY + 10) as u32;
        for i in 0..total {
            producer.publish(5, &maki_status(i));
        }
        producer.flush();

        let reader = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&reader, StartAt::Earliest);
        let mut collector = Collector { records: Vec::new() };
        let drained = consumer.read_all(&reader, &mut collector);
        // Accepted plus dropped equals what the producer published.
        assert_eq!((drained as u64 + consumer.dropped) as u32, total);
        assert_eq!(consumer.laps, 1);
        // Every accepted record decodes and carries a live sequence.
        for (id, body) in &collector.records {
            assert_eq!(*id, 5);
            assert!(MakiStatus::copy_from_bytes(body).is_some());
        }
        // The resync landed at the oldest live slot and read through the
        // producer's cursor, so the consumer is fully caught up.
        assert_eq!(consumer.next, reader.published());
    }

    #[test]
    fn heartbeat_and_status_are_visible_to_consumers() {
        let path = unique_path("health");
        let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
        ring.update_status(Status::Running);
        let beat = monotonic_nanos();
        ring.update_heartbeat(beat);

        let reader = Ring::open(&path).expect("open");
        assert_eq!(reader.peer_health(), (Status::Running, beat));
    }

    #[test]
    fn created_ring_is_unlinked_on_drop() {
        let path = unique_path("unlink");
        {
            let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
            let mut producer = Producer::new(&ring);
            producer.publish(5, &maki_status(1));
            producer.flush();
        }
        assert!(Ring::open(&path).is_err());
        assert!(!std::path::Path::new(&path).exists());
    }

    #[test]
    fn session_ring_paths_finds_created_ring() {
        let path = format!("{}/maki-{}.brng", ring_dir(), u32::MAX);
        let ring = Ring::create(&path, MAKI_NODE_ID, 0).expect("create");
        assert!(session_ring_paths().contains(&path));
        drop(ring);
        assert!(!session_ring_paths().contains(&path));
    }

    #[test]
    fn open_refuses_a_non_ring_file() {
        let path = unique_path("junk");
        std::fs::write(&path, b"not a ring at all").expect("write");
        assert!(matches!(Ring::open(&path), Err(RingError::Truncated)));
        let _ = std::fs::remove_file(&path);
    }
}
