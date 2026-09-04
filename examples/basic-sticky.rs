use std::thread;

use fragile::Sticky;

fn main() {
    // creating and using a sticky object in the same thread works
    let val = Sticky::new(true);
    println!("debug print in same thread: {:?}", val);
    println!(
        "try_with in same thread: {:?}",
        val.try_with(|value| *value)
    );

    // once sent to another thread it stops working
    thread::spawn(move || {
        println!("debug print in other thread: {:?}", val);
        println!(
            "try_with in other thread: {:?}",
            val.try_with(|value| *value)
        );
    })
    .join()
    .unwrap();
}
