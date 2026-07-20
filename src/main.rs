use std::{
    collections::BTreeMap,
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
    let mut station_stats = BTreeMap::<String, Station>::new();
    for line in f.lines() {
        let line = line.unwrap();
        let (station, temp) = line.split_once(";").unwrap();
        let temp: f64 = temp.parse().unwrap();
        let station_entry = station_stats.entry(station.to_string()).or_insert(Station {
            min_temp: f64::MAX,
            max_temp: f64::MIN,
            sum: 0.0,
            count: 0,
        });
        station_entry.min_temp = station_entry.min_temp.min(temp);
        station_entry.max_temp = station_entry.max_temp.max(temp);
        station_entry.sum += temp;
        station_entry.count += 1;
    }

    print!("{{");
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
