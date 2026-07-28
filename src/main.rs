#![feature(hasher_prefixfree_extras)]
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap, hash_map::Entry},
    ffi::{c_int, c_void},
    fs::File,
    hash::{BuildHasher, Hash, Hasher},
    io::{self},
    os::fd::AsRawFd,
};

struct Station {
    min_temp: i16,
    max_temp: i16,
    sum: i64,
    count: usize,
}

const HASH_K: u64 = 0x29075a5bfdefefd6;
const HASH_SEED: u64 = 0xb439cb55ce7d4c61;

struct HasherBuilder;
struct MyHasher(u64);

impl BuildHasher for HasherBuilder {
    type Hasher = MyHasher;

    fn build_hasher(&self) -> Self::Hasher {
        MyHasher(0xd5c937d8175a6bf4)
    }
}

impl Hasher for MyHasher {
    fn finish(&self) -> u64 {
        self.0.rotate_left(26)
    }

    fn write_length_prefix(&mut self, _len: usize) {}

    fn write(&mut self, bytes: &[u8]) {
        let len = bytes.len();
        let mut acc = HASH_SEED;

        match len {
            0..4 => {
                let low = bytes[0];
                let mid = bytes[len / 2];
                let high = bytes[len - 1];
                acc ^= (low as u64) | ((mid as u64) << 8) | ((high as u64) << 16);
            }
            4.. => {
                acc ^= u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as u64;
            }
        }

        self.0 = self.0.wrapping_add(acc).wrapping_mul(HASH_K);
    }
}

const INLINE: usize = 16;
const LAST: usize = INLINE - 1;

#[repr(C)]
union InlinedVec {
    inlined: [u8; INLINE],
    heap: (*mut u8, usize),
}

impl InlinedVec {
    pub fn new(bytes: &[u8]) -> Self {
        if bytes.len() < INLINE {
            let mut combined = [0u8; INLINE];
            combined[..bytes.len()].copy_from_slice(bytes);
            combined[LAST] = bytes.len() as u8;
            Self { inlined: combined }
        } else {
            std::hint::cold_path();
            let (ptr, len, _cap) = bytes.to_vec().into_raw_parts();
            Self { heap: (ptr, len) }
        }
    }
}

impl Drop for InlinedVec {
    fn drop(&mut self) {
        unsafe {
            if self.inlined[LAST] == 0x00 {
                let _ = Vec::from_raw_parts(self.heap.0, self.heap.1, self.heap.1);
            }
        }
    }
}

impl PartialEq for InlinedVec {
    fn eq(&self, other: &Self) -> bool {
        unsafe {
            self.inlined[LAST] == other.inlined[LAST] && {
                std::hint::cold_path();
                self.as_ref() == other.as_ref()
            }
        }
    }
}

impl Eq for InlinedVec {}

// SAFETY: Just a Vec<str> which is fine across thread boundries.
unsafe impl Send for InlinedVec {}

impl Hash for InlinedVec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl AsRef<[u8]> for InlinedVec {
    fn as_ref(&self) -> &[u8] {
        unsafe {
            if self.inlined[LAST] != 0x00 {
                let len = self.inlined[LAST] as usize;
                std::slice::from_raw_parts(self.inlined.as_ptr(), len)
            } else {
                std::hint::cold_path();
                let len = self.heap.1;
                let ptr = self.heap.0;
                std::slice::from_raw_parts(ptr, len)
            }
        }
    }
}

impl Borrow<[u8]> for InlinedVec {
    fn borrow(&self) -> &[u8] {
        self.as_ref()
    }
}

fn main() {
    let f = File::open("measurements.txt").unwrap();
    let mut station_stats =
        HashMap::<InlinedVec, Station, _>::with_capacity_and_hasher(512, HasherBuilder);
    std::thread::scope(|scope| {
        let map = mmap(&f);
        let thread_count = std::thread::available_parallelism().unwrap().get();
        let (tx, rx) = std::sync::mpsc::sync_channel(thread_count);
        let chunk_size = map.len() / thread_count;
        let mut at = 0;

        for _ in 0..thread_count {
            let start = at;
            let end = (at + chunk_size).min(map.len());
            let newline_offset = next_newline(&map[end..]);
            let end = if end == map.len() {
                std::hint::cold_path();
                end
            } else {
                end + newline_offset + 1
            };
            let map = &map[start..end];
            at = end;
            let tx = tx.clone();
            scope.spawn(move || {
                let _ = tx.send(compute_shard(map));
            });
        }

        drop(tx);
        for stats in rx {
            for (k, v) in stats {
                match station_stats.entry(k) {
                    Entry::Vacant(none) => {
                        none.insert(v);
                    }
                    Entry::Occupied(some) => {
                        let stat = some.into_mut();
                        stat.min_temp = stat.min_temp.min(v.min_temp);
                        stat.max_temp = stat.max_temp.max(v.max_temp);
                        stat.sum += v.sum;
                        stat.count += v.count;
                    }
                }
            }
        }
    });

    print!("{{");
    let station_stats = BTreeMap::from_iter(
        station_stats
            .iter()
            // SAFETY: station names are valid UTF-8 as per README.
            .map(|(k, v)| (unsafe { str::from_utf8_unchecked(k.as_ref()) }, v)),
    );
    let mut station_stats = station_stats.into_iter().peekable();
    while let Some((station_name, station_details)) = station_stats.next() {
        print!(
            "{station_name}={:.1}/{:.1}/{:.1}",
            station_details.min_temp as f64 / 10.,
            (station_details.sum as f64 / 10.) / station_details.count as f64,
            station_details.max_temp as f64 / 10.
        );
        if station_stats.peek().is_some() {
            print!(", ");
        }
    }
    print!("}}");
}

fn next_newline(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        std::hint::cold_path();
        return 0;
    }
    let newline =
        // SAFETY: bytes is a valid pointer to map and \n is promised in every line in README.
        unsafe { libc::memchr(bytes.as_ptr() as *const c_void, b'\n' as c_int, bytes.len()) };
    let offset = unsafe { (newline as *const u8).offset_from(bytes.as_ptr()) } as usize;
    offset
}

fn compute_shard(map: &[u8]) -> HashMap<InlinedVec, Station, HasherBuilder> {
    let mut station_stats =
        HashMap::<InlinedVec, Station, _>::with_capacity_and_hasher(512, HasherBuilder);
    let mut at = 0;
    loop {
        let rest = &map[at..];
        if rest.is_empty() {
            break;
        }

        let delimiter =
            // SAFETY: rest is a valid pointer to map and ; is promised in every line in README.
            unsafe { libc::memchr(rest.as_ptr() as *const c_void, b';' as c_int, rest.len()) };
        let station_len = unsafe { (delimiter as *const u8).offset_from(rest.as_ptr()) } as usize;
        let station = &rest[..station_len];
        at += station_len + 1;

        let (t, bytes_read) = parse_temperature(&map[at..]);
        at += bytes_read;

        let station_entry = match station_stats.get_mut(station) {
            Some(entry) => entry,
            None => station_stats
                .entry(InlinedVec::new(station))
                .or_insert(Station {
                    min_temp: i16::MAX,
                    max_temp: i16::MIN,
                    sum: 0,
                    count: 0,
                }),
        };
        station_entry.min_temp = station_entry.min_temp.min(t);
        station_entry.max_temp = station_entry.max_temp.max(t);
        station_entry.sum += i64::from(t);
        station_entry.count += 1;
    }

    station_stats
}

fn parse_temperature(bytes: &[u8]) -> (i16, usize) {
    assert!(bytes.len() >= 3);
    let mut ptr = 0;
    let neg = if bytes[0] == b'-' {
        ptr += 1;
        true
    } else {
        false
    };

    let mut temp: i16 = (bytes[ptr] - b'0') as i16;
    ptr += 1;

    if bytes[ptr] != b'.' {
        temp = temp * 10 + (bytes[ptr] - b'0') as i16;
        ptr += 1;
    }
    ptr += 1;

    temp = temp * 10 + (bytes[ptr] - b'0') as i16;
    ptr += 1;
    ptr += if ptr == bytes.len() { 0 } else { 1 };

    if neg {
        temp = -temp;
    }

    (temp, ptr)
}

fn mmap(f: &File) -> &'_ [u8] {
    let len = f.metadata().unwrap().len();
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len as libc::size_t,
            libc::PROT_READ,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        )
    };

    if ptr == libc::MAP_FAILED {
        panic!("{:?}", io::Error::last_os_error());
    } else {
        if unsafe { libc::madvise(ptr, len as libc::size_t, libc::MADV_SEQUENTIAL) } != 0 {
            panic!("{:?}", io::Error::last_os_error());
        }
        unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }
    }
}
