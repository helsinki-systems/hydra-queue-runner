use nix_utils::{self, BaseStore as _};

fn main() {
    let store = nix_utils::LocalStore::init();
    let nix_prefix = nix_utils::get_nix_prefix();
    println!(
        "storepath={nix_prefix} valid={}",
        store.is_valid_path(&nix_prefix)
    );
}
