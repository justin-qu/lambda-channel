use std::thread;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};

fn spawn_thread<T: Send + 'static, U: Send + 'static, V: Clone + Send + 'static>(
    input_channel: Receiver<T>,
    output_channel: Sender<U>,
    shared_resource: V,
    function: fn(&V, T) -> U,
) {
    thread::spawn(move || loop {
        output_channel
            .send(function(&shared_resource, input_channel.recv().unwrap()))
            .unwrap();
    });
}

pub fn new_simplified_lamda_channel<
    T: Send + 'static,
    U: Send + 'static,
    V: Clone + Send + 'static,
>(
    threads: usize,
    shared_resource: V,
    function: fn(&V, T) -> U,
) -> (Sender<T>, Receiver<U>) {
    let (out_tx, rx) = unbounded();
    let (tx, in_rx) = unbounded();

    for _ in 0..threads - 1 {
        spawn_thread(
            in_rx.clone(),
            out_tx.clone(),
            shared_resource.clone(),
            function,
        );
    }

    spawn_thread(in_rx, out_tx, shared_resource, function);

    (tx, rx)
}

fn fast_lambda(_: &Option<()>, x: u32) -> u32 {
    let y = 2 * x;
    println!("Hello World! 2 * {} = {}", x, y);
    y
}

fn main() {
    let (tx, rx) = new_simplified_lamda_channel(2, None, fast_lambda);

    for i in 0..5 {
        println!("Sending {}", i);
        tx.send(i).unwrap();
        thread::sleep(Duration::from_millis(1));
    }

    for _ in 0..5 {
        println!("Receiving {}", rx.recv().unwrap());
    }
}
