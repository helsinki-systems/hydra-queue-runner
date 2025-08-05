use std::io::Read;

use nix_utils::{self, BaseStore as _};

fn main() {
    let store = nix_utils::LocalStore::init();

    let file = std::fs::File::open("/tmp/test3.nar").unwrap();
    let mut reader = std::io::BufReader::new(file);

    println!("Importing test.nar == 5g60vyp4cbgwl12pav5apyi571smp62s-hello-2.12.2.drv");
    let closure = move |_: &tokio::runtime::Runtime, data: &mut [u8]| {
        let read = reader.read(data).unwrap_or(0);
        println!("read={read} data_len={}", data.len());
        read
    };

    store.import_paths_with_cb(closure, false).unwrap();
}
