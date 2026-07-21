use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader},
};

struct Station {
    min_temp: f64,
    max_temp: f64,
    sum: f64,
    count: usize,
}

fn main() {
    let f = File::open("measurements.txt").unwrap();
    let f = BufReader::new(f);
    let mut station_stats = HashMap::<Vec<u8>, Station>::new();
    for line in f.split(b'\n') {
        let line = line.unwrap();
        let mut fields = line.rsplitn(2, |c| *c == b';');
        let temp = fields.next().unwrap();
        let station = fields.next().unwrap();
        let temp: f64 = unsafe { std::str::from_utf8_unchecked(temp).parse().unwrap() };
        let station_entry = match station_stats.get_mut(station) {
            Some(entry) => entry,
            None => station_stats.entry(station.to_vec()).or_insert(Station {
                min_temp: f64::MAX,
                max_temp: f64::MIN,
                sum: 0.0,
                count: 0,
            }),
        };
        station_entry.min_temp = station_entry.min_temp.min(temp);
        station_entry.max_temp = station_entry.max_temp.max(temp);
        station_entry.sum += temp;
        station_entry.count += 1;
    }

    print!("{{");
    let station_stats = BTreeMap::from_iter(
        station_stats
            .into_iter()
            .map(|(k, v)| (unsafe { String::from_utf8_unchecked(k) }, v)),
    );
    let mut station_stats = station_stats.into_iter().peekable();
    while let Some((station_name, station_details)) = station_stats.next() {
        print!(
            "{station_name}={:.1}/{:.1}/{:.1}",
            station_details.min_temp,
            station_details.sum / (station_details.count as f64),
            station_details.max_temp
        );
        if station_stats.peek().is_some() {
            print!(", ");
        }
    }
    print!("}}");
}
