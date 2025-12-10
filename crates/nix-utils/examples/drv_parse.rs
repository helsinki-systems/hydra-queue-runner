#[tokio::main]
async fn main() {
    let store = nix_utils::LocalStore::init();
    let drv = nix_utils::query_drv(
        &store,
        &nix_utils::StorePath::new("p3ddx202bjzs1cp8hcb5b0aip18klnc4-hello-2.12.2.drv"),
    )
    .await
    .unwrap();

    println!("{drv:?}");
}
