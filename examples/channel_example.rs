use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{char, thread};

use lambda_channel::new_lambda_channel;

fn process_file(
    total_counter: &Arc<HashMap<char, AtomicU64>>,
    filepath: &str,
) -> Result<(), String> {
    let path = Path::new(filepath);
    if let Ok(file) = File::open(path) {
        let lines = BufReader::new(file).lines();

        let mut local_counter: HashMap<char, u64> = HashMap::new();

        for line in lines.map_while(Result::ok) {
            for c in line.chars() {
                if c.is_ascii_alphanumeric() {
                    *local_counter.entry(c).or_insert(0) += 1;
                }
            }
        }

        for (k, v) in local_counter {
            if let Some(atomic_v) = total_counter.get(&k) {
                atomic_v.fetch_add(v, Ordering::Relaxed);
            }
        }

        Ok(())
    } else {
        Err(filepath.to_string())
    }
}

fn main() {
    let clock = quanta::Clock::new();
    let start = clock.now();

    let mut map: HashMap<char, AtomicU64> = HashMap::new();
    let mut all_alphanumeric: Vec<char> = Vec::new();
    all_alphanumeric.extend('0'..='9');
    all_alphanumeric.extend('a'..='z');
    all_alphanumeric.extend('A'..='Z');
    for char in all_alphanumeric {
        map.insert(char, AtomicU64::new(0));
    }
    let char_counts = Arc::new(map);

    let (tx, rx, thread_pool) = new_lambda_channel(None, None, char_counts.clone(), process_file);
    thread_pool.set_pool_size(4).unwrap();

    let files = vec![
        "./a.txt", "./b.txt", "./c.txt", "./d.txt", "./e.txt", "./f.txt",
    ];

    thread::spawn(move || {
        for file in files {
            tx.send(file).unwrap();
        }
    });

    while let Ok(msg) = rx.recv() {
        if let Err(e) = msg {
            println!("Failed to open file: {}", e);
        }
    }

    let mut total_counts: HashMap<char, u64> = HashMap::new();
    for (k, v) in char_counts.iter() {
        total_counts.insert(*k, v.load(Ordering::Relaxed));
    }

    println!("Execution Time: {:?}", start.elapsed());
    println!("{:?}", total_counts);
}
