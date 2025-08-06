#[tokio::main]
async fn main() {
    let drv = nix_utils::query_drv(&nix_utils::StorePath::new(
        "3nb2yap7yhyc5bsdddikk8kvimw2h17f-aws-sdk-cpp-1.11.448.drv",
    ))
    .await
    .unwrap();

    println!("{drv:?}");
}
