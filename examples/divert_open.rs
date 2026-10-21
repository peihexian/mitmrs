use windivert::prelude::*;

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_else(|| "true".into());
    println!("filter: {filter}");
    match WinDivert::network(&filter, 0, WinDivertFlags::new()) {
        Ok(_) => println!("opened"),
        Err(e) => println!("error: {e:?}"),
    }
}
