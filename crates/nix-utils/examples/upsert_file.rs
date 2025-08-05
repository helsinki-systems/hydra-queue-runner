// requires env vars: AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY

fn main() {
    let store =
        nix_utils::RemoteStore::init("s3://store?region=unknown&endpoint=http://localhost:9000");
    nix_utils::set_verbosity(1);
    let res = store.upsert_file(
        "log/z4zxibgvmk4ikarbbpwjql21wjmdvy85-dbus-1.drv",
        "test 123",
        "text/plain; charset=utf-8",
    );

    println!("upsert res={res:?}",);

    let stats = store.get_s3_stats().unwrap();
    println!("stats {stats:?}",);
}
