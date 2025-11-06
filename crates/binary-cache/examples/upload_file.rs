use binary_cache::S3BinaryCacheClient;
use nix_utils::BaseStore as _;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _tracing_guard = hydra_tracing::init()?;
    let local = nix_utils::LocalStore::init();
    let client = S3BinaryCacheClient::new(
        format!(
            "s3://store2?region=unknown&endpoint=http://localhost:9000&scheme=http&write-nar-listing=1&compression=zstd&ls-compression=br&log-compression=br&secret-key={}/../../example-secret-key&profile=local_nix_store",
            env!("CARGO_MANIFEST_DIR")
        ).parse()?,
    )
    .await?;
    log::info!("{:#?}", client.cfg);

    let paths_to_copy = local
        .query_requisites(
            &[&nix_utils::StorePath::new(
                "/nix/store/m1r53pnnm6hnjwyjmxska24y8amvlpjp-hello-2.12.1",
            )],
            true,
        )
        .await
        .unwrap_or_default();

    client.copy_paths(&local, paths_to_copy, true).await?;

    let stats = client.s3_stats();
    log::info!(
        "stats: put={}, put_bytes={}, put_time_ms={}, get={}, get_bytes={}, get_time_ms={}, head={}",
        stats.put,
        stats.put_bytes,
        stats.put_time_ms,
        stats.get,
        stats.get_bytes,
        stats.get_time_ms,
        stats.head
    );

    Ok(())
}
