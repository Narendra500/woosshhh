use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap},
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

struct HasherBuilder;
struct MyHasher(u64);

impl BuildHasher for HasherBuilder {
    type Hasher = MyHasher;

    fn build_hasher(&self) -> Self::Hasher {
        MyHasher(0xcbf29ce484222325)
    }
}

impl Hasher for MyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (chunks, remainder) = bytes.as_chunks::<8>();
        let mut last = [1u8; 8];
        (last[..remainder.len()]).copy_from_slice(remainder);
        for &chunk in chunks.iter().chain(std::iter::once(&last)) {
            let mixed = self.0 as i128 * (i64::from_ne_bytes(chunk) as i128 * -7046029254386353131);
            self.0 = (mixed >> 64) as u64 ^ mixed as u64;
        }
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
                let _ = self.heap.0;
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
    let map = mmap(&f);
    let mut station_stats =
        HashMap::<InlinedVec, Station, _>::with_capacity_and_hasher(10_0000, HasherBuilder);
    let mut at = 0;
    loop {
        let rest = &map[at..];
        if rest.is_empty() {
            break;
        }

        let delimiter =
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

    print!("{{");
    let station_stats = BTreeMap::from_iter(
        station_stats
            .iter()
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

fn parse_temperature(bytes: &[u8]) -> (i16, usize) {
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
    ptr += 2;

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
