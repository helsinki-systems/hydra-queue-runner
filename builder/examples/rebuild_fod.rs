#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = nix_utils::LocalStore::init();

    let p = nix_utils::query_drv(
        &store,
        &nix_utils::StorePath::new(
            "/nix/store/g32rv8s2yjfaf2815msr7b2kclylzjml-mariadb-11.4.8.tar.gz.drv",
        ),
    )
    .await?
    .unwrap();
    match builder::utils::rebuild_fod(&store, &p, None).await? {
        builder::utils::FodResult::Ok {
            actual_sri,
            output: _,
        } => {
            println!("No missmatch found: {actual_sri}")
        }
        builder::utils::FodResult::HashMismatch {
            expected_sri,
            actual_sri,
            output,
        } => {
            eprintln!("Hash missmatch:\nexpected: {expected_sri}\nactual:   {actual_sri}\n");
            eprintln!("{output}");
        }
        builder::utils::FodResult::BuildFailure {
            expected_sri: _,
            output,
        } => {
            eprintln!("Build failure\n");
            eprintln!("{output}");
        }
    }
    Ok(())
}
